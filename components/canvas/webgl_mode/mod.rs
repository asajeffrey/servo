/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

mod inprocess;
pub use self::inprocess::WebGLComm;
pub use self::inprocess::WebGLExternalImages;
pub(crate) use self::inprocess::WebGLSurfaceBackedFramebuffer;
pub(crate) use self::inprocess::WebGLSurfaceBackedFramebufferId;
