/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#![deny(unsafe_code)]

use euclid::default::Size2D;
use std::collections::HashMap;
use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use surfman::Adapter;
use surfman::Connection;
use surfman::Context;
use surfman::ContextAttributes;
use surfman::Error;
use surfman::Device;
use surfman::NativeWidget;
use surfman::SurfaceAccess;
use surfman::SurfaceType;
use surfman::Surface;
use surfman::SurfaceTexture;
use webrender_api::units::TexelRect;

/// This trait is used as a bridge between the different GL clients
/// in Servo that handles WebRender ExternalImages and the WebRender
/// ExternalImageHandler API.
//
/// This trait is used to notify lock/unlock messages and get the
/// required info that WR needs.
pub trait WebrenderExternalImageApi {
    fn lock(&mut self, id: u64) -> (u32, Size2D<i32>);
    fn unlock(&mut self, id: u64);
}

/// Type of Webrender External Image Handler.
pub enum WebrenderImageHandlerType {
    WebGL,
    Media,
}

/// List of Webrender external images to be shared among all external image
/// consumers (WebGL, Media).
/// It ensures that external image identifiers are unique.
pub struct WebrenderExternalImageRegistry {
    /// Map of all generated external images.
    external_images: HashMap<webrender_api::ExternalImageId, WebrenderImageHandlerType>,
    /// Id generator for the next external image identifier.
    next_image_id: u64,
}

impl WebrenderExternalImageRegistry {
    pub fn new() -> Self {
        Self {
            external_images: HashMap::new(),
            next_image_id: 0,
        }
    }

    pub fn next_id(
        &mut self,
        handler_type: WebrenderImageHandlerType,
    ) -> webrender_api::ExternalImageId {
        self.next_image_id += 1;
        let key = webrender_api::ExternalImageId(self.next_image_id);
        self.external_images.insert(key, handler_type);
        key
    }

    pub fn remove(&mut self, key: &webrender_api::ExternalImageId) {
        self.external_images.remove(key);
    }

    pub fn get(&self, key: &webrender_api::ExternalImageId) -> Option<&WebrenderImageHandlerType> {
        self.external_images.get(key)
    }
}

/// WebRender External Image Handler implementation.
pub struct WebrenderExternalImageHandlers {
    /// WebGL handler.
    webgl_handler: Option<Box<dyn WebrenderExternalImageApi>>,
    /// Media player handler.
    media_handler: Option<Box<dyn WebrenderExternalImageApi>>,
    /// Webrender external images.
    external_images: Arc<Mutex<WebrenderExternalImageRegistry>>,
}

impl WebrenderExternalImageHandlers {
    pub fn new() -> (Self, Arc<Mutex<WebrenderExternalImageRegistry>>) {
        let external_images = Arc::new(Mutex::new(WebrenderExternalImageRegistry::new()));
        (
            Self {
                webgl_handler: None,
                media_handler: None,
                external_images: external_images.clone(),
            },
            external_images,
        )
    }

    pub fn set_handler(
        &mut self,
        handler: Box<dyn WebrenderExternalImageApi>,
        handler_type: WebrenderImageHandlerType,
    ) {
        match handler_type {
            WebrenderImageHandlerType::WebGL => self.webgl_handler = Some(handler),
            WebrenderImageHandlerType::Media => self.media_handler = Some(handler),
        }
    }
}

impl webrender_api::ExternalImageHandler for WebrenderExternalImageHandlers {
    /// Lock the external image. Then, WR could start to read the
    /// image content.
    /// The WR client should not change the image content until the
    /// unlock() call.
    fn lock(
        &mut self,
        key: webrender_api::ExternalImageId,
        _channel_index: u8,
        _rendering: webrender_api::ImageRendering,
    ) -> webrender_api::ExternalImage {
        let external_images = self.external_images.lock().unwrap();
        let handler_type = external_images
            .get(&key)
            .expect("Tried to get unknown external image");
        let (texture_id, uv) = match handler_type {
            WebrenderImageHandlerType::WebGL => {
                let (texture_id, size) = self.webgl_handler.as_mut().unwrap().lock(key.0);
                (
                    texture_id,
                    TexelRect::new(0.0, size.height as f32, size.width as f32, 0.0),
                )
            },
            WebrenderImageHandlerType::Media => {
                let (texture_id, size) = self.media_handler.as_mut().unwrap().lock(key.0);
                (
                    texture_id,
                    TexelRect::new(0.0, 0.0, size.width as f32, size.height as f32),
                )
            },
        };
        webrender_api::ExternalImage {
            uv,
            source: webrender_api::ExternalImageSource::NativeTexture(texture_id),
        }
    }

    /// Unlock the external image. The WR should not read the image
    /// content after this call.
    fn unlock(&mut self, key: webrender_api::ExternalImageId, _channel_index: u8) {
        let external_images = self.external_images.lock().unwrap();
        let handler_type = external_images
            .get(&key)
            .expect("Tried to get unknown external image");
        match handler_type {
            WebrenderImageHandlerType::WebGL => self.webgl_handler.as_mut().unwrap().unlock(key.0),
            WebrenderImageHandlerType::Media => self.media_handler.as_mut().unwrap().unlock(key.0),
        };
    }
}

/// A bridge between webrender and surfman
// TODO: move this into a different crate so that script doesn't depend on surfman
#[derive(Clone)]
pub struct WebrenderSurfman(Rc<WebrenderSurfmanData>);

struct WebrenderSurfmanData {
    device: Device,
    mutable: RefCell<WebrenderSurfmanMutable>,
}

struct WebrenderSurfmanMutable {
    context: Context,
    render_surface: Surface,
}

impl Drop for WebrenderSurfmanData {
    fn drop(&mut self) {
        let ref mut mutable = *self.mutable.borrow_mut();
        let _ = self.device.destroy_surface(&mut mutable.context, &mut mutable.render_surface);
        let _ = self.device.destroy_context(&mut mutable.context);
    }
}

impl WebrenderSurfman {
    pub fn create(connection: &Connection, adapter: &Adapter, context_attributes: ContextAttributes, native_widget: NativeWidget) -> Result<Self, Error> {
        let mut device = connection.create_device(&adapter)?;
	let context_descriptor = device.create_context_descriptor(&context_attributes)?;
        let context = device.create_context(&context_descriptor)?;
        let surface_access = SurfaceAccess::GPUOnly;
        let surface_type = SurfaceType::Widget { native_widget };
	let render_surface = device.create_surface(&context, surface_access, surface_type)?;
	let mutable = RefCell::new(WebrenderSurfmanMutable { context, render_surface });
        Ok(WebrenderSurfman(Rc::new(WebrenderSurfmanData { device, mutable })))
    }

    pub fn create_surface_texture(&self, surface: Surface) -> Result<SurfaceTexture, (Error, Surface)> {
        let mut mutable = self.0.mutable.borrow_mut();
        self.0.device.create_surface_texture(&mut mutable.context, surface)
    }

    pub fn destroy_surface_texture(&self, surface_texture: SurfaceTexture) -> Result<Surface, (Error, SurfaceTexture)> {
        let mut mutable = self.0.mutable.borrow_mut();
        self.0.device.destroy_surface_texture(&mut mutable.context, surface_texture)
    }

    pub fn make_gl_context_current(&self) -> Result<(), Error> {
        let mutable = self.0.mutable.borrow();
        self.0.device.make_context_current(&mutable.context)
    }

    pub fn present(&self) -> Result<(), Error> {
        let ref mut mutable = *self.0.mutable.borrow_mut();
        self.0.device.present_surface(&mutable.context, &mut mutable.render_surface)
    }

    pub fn get_proc_address(&self, name: &str) -> *const c_void {
        let mutable = self.0.mutable.borrow();
	self.0.device.get_proc_address(&mutable.context, name)
    }

    pub fn device(&self) -> &Device {
        &self.0.device
    }
}
