/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::webgl_thread::{WebGLThread, WebGLThreadInit};
use crate::webgl_mode::buffers::FrontBuffers;
use atom::Atom;
use canvas_traits::webgl::{
    webgl_channel, WebGLContextId, WebGLFramebufferId, WebGLMsg, WebGLOpaqueFramebufferId,
    WebGLSender, WebGLThreads, WebVRRenderHandler,
};
use euclid::default::Size2D;
use fnv::FnvHashMap;
use gleam::gl;
use servo_config::pref;
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::default::Default;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use surfman::{self, Context, Device, Surface, SurfaceTexture};
use webrender_traits::{WebrenderExternalImageApi, WebrenderExternalImageRegistry};
use webxr_api::WebGLExternalImageApi as WebXRExternalImageApi;

pub struct WebGLComm {
    pub webgl_threads: WebGLThreads,
    pub webxr_handler: Arc<dyn Fn() -> Box<dyn WebXRExternalImageApi> + Send + Sync>,
    pub image_handler: Box<dyn WebrenderExternalImageApi>,
    pub output_handler: Option<Box<dyn webrender::OutputImageHandler>>,
}

impl WebGLComm {
    /// Creates a new `WebGLComm` object.
    pub fn new(
        device: Device,
        context: Context,
        webrender_gl: Rc<dyn gl::Gl>,
        webrender_api_sender: webrender_api::RenderApiSender,
        webvr_compositor: Option<Box<dyn WebVRRenderHandler>>,
        external_images: Arc<Mutex<WebrenderExternalImageRegistry>>,
    ) -> WebGLComm {
        unimplemented!()
        /*
            println!("WebGLThreads::new()");
            let (sender, receiver) = webgl_channel::<WebGLMsg>().unwrap();

            // This implementation creates a single `WebGLThread` for all the pipelines.
            let init = WebGLThreadInit {
                webrender_api_sender,
                webvr_compositor,
                external_images,
                sender: sender.clone(),
                receiver,
                adapter: device.adapter(),
            };

            let output_handler = if pref!(dom.webgl.dom_to_texture.enabled) {
                Some(Box::new(OutputHandler::new(
                    webrender_gl.clone(),
                    sender.clone(),
                )))
            } else {
                None
            };

            let external = WebGLExternalImages::new(device,
                                                    context,
                                                    webrender_gl,
                                                    front_buffer,
                                                    sender.clone());

            WebGLThread::run_on_own_thread(init);

            WebGLComm {
                webgl_threads: WebGLThreads(sender),
                webxr_handler: external.sendable().clone_box(),
                image_handler: Box::new(external),
                output_handler: output_handler.map(|b| b as Box<_>),
            }
        */
    }
}

/// Bridge between the webrender::ExternalImage callbacks and the WebGLThreads.
pub struct WebGLExternalImages {
    device: Device,
    context: Context,
    front_buffers: FrontBuffers,
    locked: Option<(
        WebGLDoubleBufferId,
        SurfaceTexture,
    )>,
}

impl WebGLExternalImages {
    pub fn new(device: Device, context: Context) -> Self {
        Self {
            device,
            context,
            surface_backed_framebuffers: Default::default(),
            locked: None,
        }
    }

    fn lock_front_buffer(
        &mut self,
        buffer_id: DoubleBufferId,
    ) -> Option<(u32, Size2D<i32>)> {
        let surface = self.front_buffers.take()?;
	let size = surface.size();
        let surface_texture = self
            .device
            .create_surface_texture(&mut self.context, surface)
            .ok()?;
	let gl_texture = surface_texture.gl_texture();
        self.unlock_front_buffer();
        self.locked = Some((buffer_id, surface_texture));
        Some((gl_texture, size))
    }

    fn unlock_front_buffer(&mut self) -> Option<()> {
        let (buffer_id, surface_texture) = self.locked.take()?;
        let surface = self
            .device
            .destroy_surface_texture(&mut self.context, surface_texture)
            .ok()?;
        // It is possible that WebGL is generating frames faster than we can render them,
        // in which case the surface box should be disposed of.
        if let Some(surface) = self.front_buffers.put(buffer_id, surface) {
            self.device
                .destroy_surface(&mut self.context, surface)
                .ok()?;
        }
        Some(())
    }
}

impl WebrenderExternalImageApi for WebGLExternalImages {
    fn lock(&mut self, id: u64) -> (u32, Size2D<i32>) {
        let buffer_id = DoubleBufferId::Default(WebGLContextId(id as usize));
        self.lock_front_buffer(buffer_id).unwrap_or_default()
    }

    fn unlock(&mut self, _id: u64) {
        self.unlock_front_buffer();
    }
}

impl WebXRExternalImageApi for WebGLExternalImages {
    fn lock(&mut self, id: u64) -> (u32, Size2D<i32>) {
        let buffer_id = DoubleBufferId::Opaque(WebGLOpaqueFramebufferId::new(id as usize));
        self.lock_front_buffer(buffer_id).unwrap_or_default()
    }

    fn unlock(&mut self, _id: NonZeroU32) {
        self.unlock_front_buffer();
    }
}

/// struct used to implement DOMToTexture feature and webrender::OutputImageHandler trait.
//type OutputHandlerData = Option<(u32, Size2D<i32>)>;
struct OutputHandler {
    webrender_gl: Rc<dyn gl::Gl>,
    webgl_channel: WebGLSender<WebGLMsg>,
    // Used to avoid creating a new channel on each received WebRender request.
    sync_objects: FnvHashMap<webrender_api::PipelineId, gl::GLsync>,
}

impl OutputHandler {
    fn new(webrender_gl: Rc<dyn gl::Gl>, channel: WebGLSender<WebGLMsg>) -> Self {
        OutputHandler {
            webrender_gl,
            webgl_channel: channel,
            sync_objects: Default::default(),
        }
    }
}

/// Bridge between the WR frame outputs and WebGL to implement DOMToTexture synchronization.
impl webrender::OutputImageHandler for OutputHandler {
    fn lock(
        &mut self,
        id: webrender_api::PipelineId,
    ) -> Option<(u32, webrender_api::units::FramebufferIntSize)> {
        // Insert a fence in the WR command queue
        let gl_sync = self
            .webrender_gl
            .fence_sync(gl::SYNC_GPU_COMMANDS_COMPLETE, 0);
        self.sync_objects.insert(id, gl_sync);
        None
    }

    fn unlock(&mut self, id: webrender_api::PipelineId) {
        if let Some(gl_sync) = self.sync_objects.remove(&id) {
            // Flush the Sync object into the GPU's command queue to guarantee that it it's signaled.
            self.webrender_gl.flush();
            // Mark the sync object for deletion.
            self.webrender_gl.delete_sync(gl_sync);
        }
    }
}
