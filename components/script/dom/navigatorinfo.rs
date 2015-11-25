/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use util::opts;
use util::str::DOMString;

pub fn Product() -> DOMString {
    DOMString::from(atom!("Gecko"))
}

pub fn TaintEnabled() -> bool {
    false
}

pub fn AppName() -> DOMString {
    DOMString::from(atom!("Netscape")) // Like Gecko/Webkit
}

pub fn AppCodeName() -> DOMString {
    DOMString::from(atom!("Mozilla"))
}

#[cfg(target_os = "windows")]
pub fn Platform() -> DOMString {
    DOMString::from(atom!("Win32"))
}

#[cfg(any(target_os = "android", target_os = "linux"))]
pub fn Platform() -> DOMString {
    DOMString::from(atom!("Linux"))
}

#[cfg(target_os = "macos")]
pub fn Platform() -> DOMString {
    DOMString::from(atom!("Mac"))
}

pub fn UserAgent() -> DOMString {
    DOMString::from(&*opts::get().user_agent)
}

pub fn AppVersion() -> DOMString {
    DOMString::from(atom!("4.0"))
}

