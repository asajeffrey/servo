/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use std::sync::Arc;
use url::Origin as UrlOrigin;
use url::{Url, Host};

/// A representation of an [origin](https://html.spec.whatwg.org/multipage/#origin-2).
#[derive(HeapSizeOf, JSTraceable)]
pub struct Origin {
    #[ignore_heap_size_of = "Arc<T> has unclear ownership semantics"]
    inner: Arc<UrlOrigin>,
}

impl Origin {
    /// Create a new origin comprising a unique, opaque identifier.
    pub fn opaque_identifier() -> Origin {
        Origin {
            inner: Arc::new(UrlOrigin::new_opaque()),
        }
    }

    /// Create a new origin for the given URL.
    pub fn new(url: &Url) -> Origin {
        Origin {
            inner: Arc::new(url.origin()),
        }
    }

    /// Does this origin represent a host/scheme/port tuple?
    pub fn is_scheme_host_port_tuple(&self) -> bool {
        self.inner.is_tuple()
    }

    /// Return the host associated with this origin.
    pub fn host(&self) -> Option<&Host<String>> {
        match *self.inner {
            UrlOrigin::Tuple(_, ref host, _) => Some(host),
            UrlOrigin::Opaque(..) => None,
        }
    }

    /// Return the domain associated with this origin.
    /// TODO: implement setting the domain.
    pub fn domain(&self) -> Option<&str> {
        None
    }

    /// https://html.spec.whatwg.org/multipage/#same-origin
    pub fn same_origin(&self, other: &Origin) -> bool {
        self.inner == other.inner
    }

    /// https://html.spec.whatwg.org/multipage/#same-origin-domain
    pub fn same_origin_domain(&self, other: &Origin) -> bool {
        match (&*self.inner, self.domain(), &*other.inner, other.domain()) {
            // Step 1.
            (&UrlOrigin::Opaque(ref opaqueA), _, &UrlOrigin::Opaque(ref opaqueB), _) =>
                opaqueA == opaqueB,
            // Step 2.1.
            (&UrlOrigin::Tuple(ref schA, _, _), Some(domA), &UrlOrigin::Tuple(ref schB, _, _), Some(domB)) =>
                (schA == sch0B) && (domA == domB),
            // Step 2.2.
            (&UrlOrigin::Tuple(_, _, _), None, &UrlOrigin::Tuple(_, _, _), None) =>
                self.same_origin(other),
            // Step 3.
            _ =>
                false,
        }
    }

    pub fn copy(&self) -> Origin {
        Origin {
            inner: Arc::new((*self.inner).clone()),
        }
    }

    pub fn alias(&self) -> Origin {
        Origin {
            inner: self.inner.clone(),
        }
    }
}
