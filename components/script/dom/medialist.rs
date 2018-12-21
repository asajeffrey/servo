/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::dom::bindings::codegen::Bindings::MediaListBinding;
use crate::dom::bindings::codegen::Bindings::MediaListBinding::MediaListMethods;
use crate::dom::bindings::codegen::Bindings::WindowBinding::WindowBinding::WindowMethods;
use crate::dom::bindings::reflector::{reflect_dom_object, DomObject, Reflector};
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::bindings::str::DOMString;
use crate::dom::cssstylesheet::CSSStyleSheet;
use crate::dom::window::Window;
use cssparser::{Parser, ParserInput};
use dom_struct::dom_struct;
use servo_arc::Arc;
use style::media_queries::MediaList as StyleMediaList;
use style::media_queries::MediaQuery;
use style::parser::ParserContext;
use style::shared_lock::{Locked, SharedRwLock};
use style::stylesheets::CssRuleType;
use style_traits::{ParsingMode, ToCss};

#[dom_struct]
pub struct MediaList {
    reflector_: Reflector,
    parent_stylesheet: Dom<CSSStyleSheet>,
    #[ignore_malloc_size_of = "Arc"]
    media_queries: Arc<Locked<StyleMediaList>>,
}

impl MediaList {
    #[allow(unrooted_must_root)]
    pub fn new_inherited(
        parent_stylesheet: &CSSStyleSheet,
        media_queries: Arc<Locked<StyleMediaList>>,
    ) -> MediaList {
        MediaList {
            parent_stylesheet: Dom::from_ref(parent_stylesheet),
            reflector_: Reflector::new(),
            media_queries: media_queries,
        }
    }

    #[allow(unrooted_must_root)]
    pub fn new(
        window: &Window,
        parent_stylesheet: &CSSStyleSheet,
        media_queries: Arc<Locked<StyleMediaList>>,
    ) -> DomRoot<MediaList> {
        reflect_dom_object(
            Box::new(MediaList::new_inherited(parent_stylesheet, media_queries)),
            window,
            MediaListBinding::Wrap,
        )
    }

    fn shared_lock(&self) -> &SharedRwLock {
        &self.parent_stylesheet.style_stylesheet().shared_lock
    }
}

impl MediaListMethods for MediaList {
    // https://drafts.csswg.org/cssom/#dom-medialist-mediatext
    fn MediaText(&self) -> DOMString {
        let future = self.shared_lock().read_async();
        let guard = self.global().await_future(future).expect("Script thread shut down during media query");
        DOMString::from(self.media_queries.read_with(&guard).to_css_string())
    }

    // https://drafts.csswg.org/cssom/#dom-medialist-mediatext
    fn SetMediaText(&self, value: DOMString) {
        let mut guard = self.shared_lock().write();
        let media_queries = self.media_queries.write_with(&mut guard);
        // Step 2
        if value.is_empty() {
            // Step 1
            *media_queries = StyleMediaList::empty();
            return;
        }
        // Step 3
        let mut input = ParserInput::new(&value);
        let mut parser = Parser::new(&mut input);
        let global = self.global();
        let window = global.as_window();
        let url = window.get_url();
        let quirks_mode = window.Document().quirks_mode();
        let context = ParserContext::new_for_cssom(
            &url,
            Some(CssRuleType::Media),
            ParsingMode::DEFAULT,
            quirks_mode,
            window.css_error_reporter(),
            None,
        );
        *media_queries = StyleMediaList::parse(&context, &mut parser);
    }

    // https://drafts.csswg.org/cssom/#dom-medialist-length
    fn Length(&self) -> u32 {
        let guard = self.shared_lock().read();
        self.media_queries.read_with(&guard).media_queries.len() as u32
    }

    // https://drafts.csswg.org/cssom/#dom-medialist-item
    fn Item(&self, index: u32) -> Option<DOMString> {
        let guard = self.shared_lock().read();
        self.media_queries
            .read_with(&guard)
            .media_queries
            .get(index as usize)
            .map(|query| query.to_css_string().into())
    }

    // https://drafts.csswg.org/cssom/#dom-medialist-item
    fn IndexedGetter(&self, index: u32) -> Option<DOMString> {
        self.Item(index)
    }

    // https://drafts.csswg.org/cssom/#dom-medialist-appendmedium
    fn AppendMedium(&self, medium: DOMString) {
        // Step 1
        let mut input = ParserInput::new(&medium);
        let mut parser = Parser::new(&mut input);
        let global = self.global();
        let win = global.as_window();
        let url = win.get_url();
        let quirks_mode = win.Document().quirks_mode();
        let context = ParserContext::new_for_cssom(
            &url,
            Some(CssRuleType::Media),
            ParsingMode::DEFAULT,
            quirks_mode,
            win.css_error_reporter(),
            None,
        );
        let m = MediaQuery::parse(&context, &mut parser);
        // Step 2
        if let Err(_) = m {
            return;
        }
        // Step 3
        let m_serialized = m.clone().unwrap().to_css_string();
        let mut guard = self.shared_lock().write();
        let mq = self.media_queries.write_with(&mut guard);
        let any = mq
            .media_queries
            .iter()
            .any(|q| m_serialized == q.to_css_string());
        if any {
            return;
        }
        // Step 4
        mq.media_queries.push(m.unwrap());
    }

    // https://drafts.csswg.org/cssom/#dom-medialist-deletemedium
    fn DeleteMedium(&self, medium: DOMString) {
        // Step 1
        let mut input = ParserInput::new(&medium);
        let mut parser = Parser::new(&mut input);
        let global = self.global();
        let win = global.as_window();
        let url = win.get_url();
        let quirks_mode = win.Document().quirks_mode();
        let context = ParserContext::new_for_cssom(
            &url,
            Some(CssRuleType::Media),
            ParsingMode::DEFAULT,
            quirks_mode,
            win.css_error_reporter(),
            None,
        );
        let m = MediaQuery::parse(&context, &mut parser);
        // Step 2
        if let Err(_) = m {
            return;
        }
        // Step 3
        let m_serialized = m.unwrap().to_css_string();
        let mut guard = self.shared_lock().write();
        let media_list = self.media_queries.write_with(&mut guard);
        let new_vec = media_list
            .media_queries
            .drain(..)
            .filter(|q| m_serialized != q.to_css_string())
            .collect();
        media_list.media_queries = new_vec;
    }
}
