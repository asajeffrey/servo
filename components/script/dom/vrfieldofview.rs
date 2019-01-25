/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::dom::bindings::codegen::Bindings::VRFieldOfViewBinding;
use crate::dom::bindings::codegen::Bindings::VRFieldOfViewBinding::VRFieldOfViewMethods;
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::num::Finite;
use crate::dom::bindings::reflector::{reflect_dom_object, Reflector};
use crate::dom::bindings::reflector::DomObject;
use crate::dom::bindings::root::DomRoot;
use crate::dom::globalscope::GlobalScope;
use crate::dom::window::Window;
use dom_struct::dom_struct;
use euclid::Angle;
use euclid::Trig;
use webvr_traits::WebVRFieldOfView;

#[dom_struct]
pub struct VRFieldOfView {
    reflector_: Reflector,
    #[ignore_malloc_size_of = "Defined in rust-webvr"]
    fov: Option<WebVRFieldOfView>,
}

unsafe_no_jsmanaged_fields!(WebVRFieldOfView);

impl VRFieldOfView {
    fn new_inherited(fov: Option<WebVRFieldOfView>) -> VRFieldOfView {
        VRFieldOfView {
            reflector_: Reflector::new(),
            fov: fov,
        }
    }

    pub fn new(global: &GlobalScope, fov: Option<WebVRFieldOfView>) -> DomRoot<VRFieldOfView> {
        reflect_dom_object(
            Box::new(VRFieldOfView::new_inherited(fov)),
            global,
            VRFieldOfViewBinding::Wrap,
        )
    }

    fn default_hfov(&self) -> Angle<f64> {
        // If the device provides no fov, then we are rendering into
        // the current window, so we use its aspect ratio
        self.global()
            .downcast::<Window>()
            .map(|window| window.window_size())
            .map(|size| size.initial_viewport.to_f64())
            .map(|size| Angle::radians(f64::fast_atan2(size.width, size.height)))
            .unwrap_or(Angle::frac_pi_4())
    }

    fn default_vfov(&self) -> Angle<f64> {
        // If the device provides no fov, then we are rendering into
        // the current window, so we use its aspect ratio
        self.global()
            .downcast::<Window>()
            .map(|window| window.window_size())
            .map(|size| size.initial_viewport.to_f64())
            .map(|size| Angle::radians(f64::fast_atan2(size.height, size.width)))
            .unwrap_or(Angle::frac_pi_4())
    }
}

impl VRFieldOfViewMethods for VRFieldOfView {
    // https://w3c.github.io/webvr/#interface-interface-vrfieldofview
    fn UpDegrees(&self) -> Finite<f64> {
        self.fov.as_ref()
            .map(|fov| Finite::wrap(fov.up_degrees))
            .unwrap_or_else(|| Finite::wrap(self.default_vfov().to_degrees()))
    }

    // https://w3c.github.io/webvr/#interface-interface-vrfieldofview
    fn RightDegrees(&self) -> Finite<f64> {
        self.fov.as_ref()
            .map(|fov| Finite::wrap(fov.right_degrees))
            .unwrap_or_else(|| Finite::wrap(self.default_hfov().to_degrees()))
    }

    // https://w3c.github.io/webvr/#interface-interface-vrfieldofview
    fn DownDegrees(&self) -> Finite<f64> {
        self.fov.as_ref()
            .map(|fov| Finite::wrap(fov.down_degrees))
            .unwrap_or_else(|| Finite::wrap(self.default_vfov().to_degrees()))
    }

    // https://w3c.github.io/webvr/#interface-interface-vrfieldofview
    fn LeftDegrees(&self) -> Finite<f64> {
        self.fov.as_ref()
            .map(|fov| Finite::wrap(fov.left_degrees))
            .unwrap_or_else(|| Finite::wrap(self.default_hfov().to_degrees()))
    }
}
