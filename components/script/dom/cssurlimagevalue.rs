/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use dom::bindings::codegen::Bindings::CSSURLImageValueBinding::CSSURLImageValueMethods;
use dom::bindings::codegen::Bindings::CSSURLImageValueBinding::Wrap;
use dom::bindings::error::Error;
use dom::bindings::error::Fallible;
use dom::bindings::js::Root;
use dom::bindings::reflector::Reflector;
use dom::bindings::reflector::reflect_dom_object;
use dom::bindings::str::USVString;
use dom::globalscope::GlobalScope;
use dom_struct::dom_struct;
use servo_url::ServoUrl;

#[dom_struct]
pub struct CSSURLImageValue {
    reflector: Reflector,
    url: ServoUrl,
}

impl CSSURLImageValue {
    fn new_inherited(url: ServoUrl) -> CSSURLImageValue {
        CSSURLImageValue {
            reflector: Reflector::new(),
            url: url,
        }
    }

    pub fn new(global: &GlobalScope, url: ServoUrl) -> Root<CSSURLImageValue> {
        reflect_dom_object(box CSSURLImageValue::new_inherited(url), global, Wrap)
    }

    /// https://drafts.css-houdini.org/css-typed-om-1/#dom-cssurlimagevalue-cssurlimagevalue
    // https://github.com/w3c/css-houdini-drafts/issues/424
    pub fn Constructor(global: &GlobalScope, url: USVString) -> Fallible<Root<CSSURLImageValue>> {
        let url = ServoUrl::parse(&*url.0)
            .map_err(|err| Error::Type(format!("Failed to parse URL ({}).", err)))?;
        Ok(CSSURLImageValue::new(global, url))
    }
}

impl CSSURLImageValueMethods for CSSURLImageValue {
    fn Url(&self) -> USVString {
        USVString(self.url.as_str().into())
    }
}
