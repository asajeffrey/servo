/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::dom::bindings::codegen::Bindings::VRDisplayBinding::VREye;
use crate::dom::bindings::codegen::Bindings::VREyeParametersBinding;
use crate::dom::bindings::codegen::Bindings::VREyeParametersBinding::VREyeParametersMethods;
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::reflector::{reflect_dom_object, Reflector, DomObject};
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::globalscope::GlobalScope;
use crate::dom::vrfieldofview::VRFieldOfView;
use crate::dom::window::Window;
use dom_struct::dom_struct;
use euclid::TypedSize2D;
use js::jsapi::{Heap, JSContext, JSObject};
use js::typedarray::{CreateWith, Float32Array};
use std::default::Default;
use std::ptr;
use std::ptr::NonNull;
use style_traits::DevicePixel;
use webvr_traits::WebVREyeParameters;

#[dom_struct]
pub struct VREyeParameters {
    reflector_: Reflector,
    #[ignore_malloc_size_of = "Defined in rust-webvr"]
    parameters: Option<WebVREyeParameters>,
    eye: VREye,
    offset: Heap<*mut JSObject>,
    fov: Dom<VRFieldOfView>,
}

unsafe_no_jsmanaged_fields!(WebVREyeParameters);

impl VREyeParameters {
    fn new_inherited(parameters: Option<WebVREyeParameters>, eye: VREye, fov: &VRFieldOfView) -> VREyeParameters {
        VREyeParameters {
            reflector_: Reflector::new(),
            parameters: parameters,
            eye: eye,
            offset: Heap::default(),
            fov: Dom::from_ref(&*fov),
        }
    }

    #[allow(unsafe_code)]
    pub fn new(parameters: Option<WebVREyeParameters>, eye: VREye, global: &GlobalScope) -> DomRoot<VREyeParameters> {
        let fov = parameters.as_ref()
            .map(|params| params.field_of_view.clone());
        let fov = VRFieldOfView::new(&global, fov);

        let offset = parameters.as_ref()
            .map(|params| params.offset)
            .unwrap_or_else(|| VREyeParameters::default_offset(eye));

        let cx = global.get_cx();
        rooted!(in (cx) let mut array = ptr::null_mut::<JSObject>());
        unsafe {
            let _ = Float32Array::create(
                cx,
                CreateWith::Slice(&offset),
                array.handle_mut(),
            );
        }

        let eye_parameters = reflect_dom_object(
            Box::new(VREyeParameters::new_inherited(parameters, eye, &fov)),
            global,
            VREyeParametersBinding::Wrap,
        );
        eye_parameters.offset.set(array.get());

        eye_parameters
    }

    fn default_offset(eye: VREye) -> [f32; 3] {
        match eye {
            VREye::Left  => [ -0.02, 0.0, 0.0 ],
            VREye::Right => [  0.02, 0.0, 0.0 ],
        }
    }

    fn default_render_size(&self) -> TypedSize2D<u32, DevicePixel> {
        // If the device provides no render width, then we are rendering into
        // the current window, so we use its width in device pixels.
        self.global()
            .downcast::<Window>()
            .map(|window| window.window_size())
            .map(|size| size.initial_viewport * size.device_pixel_ratio)
            .map(|size| size.to_u32())
            .unwrap_or(TypedSize2D::zero())
    }
}

impl VREyeParametersMethods for VREyeParameters {
    #[allow(unsafe_code)]
    // https://w3c.github.io/webvr/#dom-vreyeparameters-offset
    unsafe fn Offset(&self, _cx: *mut JSContext) -> NonNull<JSObject> {
        NonNull::new_unchecked(self.offset.get())
    }

    // https://w3c.github.io/webvr/#dom-vreyeparameters-fieldofview
    fn FieldOfView(&self) -> DomRoot<VRFieldOfView> {
        DomRoot::from_ref(&*self.fov)
    }

    // https://w3c.github.io/webvr/#dom-vreyeparameters-renderwidth
    fn RenderWidth(&self) -> u32 {
        self.parameters.as_ref()
            .map(|params| params.render_width)
            .unwrap_or_else(|| self.default_render_size().width)
    }

    // https://w3c.github.io/webvr/#dom-vreyeparameters-renderheight
    fn RenderHeight(&self) -> u32 {
        self.parameters.as_ref()
            .map(|params| params.render_width)
            .unwrap_or_else(|| self.default_render_size().width)
    }
}
