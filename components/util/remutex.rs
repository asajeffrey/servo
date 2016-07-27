/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! An implementation of re-entrant mutexes.
//!
//! Re-entrant mutexes are like mutexes, but where it is expected
//! that a single thread may own a lock more than once.

//! It provides the same interface as https://github.com/rust-lang/rust/blob/master/src/libstd/sys/common/remutex.rs
//! so if those types are ever exported, we should be able to replace this implemtation.

use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, LockResult, Mutex, MutexGuard, PoisonError, TryLockError, TryLockResult};

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct ThreadId(usize);

lazy_static!{ static ref THREAD_COUNT: AtomicUsize = AtomicUsize::new(0); }

impl ThreadId {
    fn new() -> ThreadId {
        ThreadId(THREAD_COUNT.fetch_add(1, Ordering::SeqCst))
    }
    pub fn current() -> ThreadId {
        THREAD_ID.with(|tls| tls.clone())
    }
}

thread_local!{ static THREAD_ID: ThreadId = ThreadId::new(); }

pub struct ReentrantMutex<T> {
    mutex: Mutex<(ThreadId, usize)>,
    condvar: Condvar,
    data: T,
}

#[allow(unsafe_code)]
unsafe impl<T> Sync for ReentrantMutex<T> where T: Send {}

impl<T> ReentrantMutex<T> {
    pub fn new(data: T) -> ReentrantMutex<T> {
        ReentrantMutex {
            mutex: Mutex::new((ThreadId::current(), 0)),
            condvar: Condvar::new(),
            data: data,
        }
    }

    fn unlock(&self) {
        if let Ok(mut locked) = self.mutex.lock() {
            locked.1 = locked.1 - 1;
            if locked.1 == 0 { self.condvar.notify_one(); }
        }
    }

    fn try_once(&self, attempt: &mut LockResult<MutexGuard<(ThreadId, usize)>>)
                -> TryLockResult<ReentrantMutexGuard<T>>
    {
        let current = ThreadId::current();
        let result = ReentrantMutexGuard { mutex: &self };
        match attempt {
            &mut Ok(ref mut locked) => if locked.0 == current {
                locked.1 = locked.1 + 1;
                Ok(result)
            } else if locked.1 == 0 {
                locked.0 = current;
                locked.1 = 1;
                Ok(result)
            } else {
                Err(TryLockError::WouldBlock)
            },
            &mut Err(_) => Err(TryLockError::Poisoned(PoisonError::new(result))),
        }
    }

    pub fn try_lock(&self) -> TryLockResult<ReentrantMutexGuard<T>> {
        let mut locked = self.mutex.lock();
        self.try_once(&mut locked)
    }

    pub fn lock(&self) -> LockResult<ReentrantMutexGuard<T>> {
        let mut locked = self.mutex.lock();
        loop {
            match self.try_once(&mut locked) {
                Ok(result) => { return Ok(result); },
                Err(TryLockError::Poisoned(err)) => { return Err(err); },
                Err(TryLockError::WouldBlock) => { locked = self.condvar.wait(locked.unwrap()); },
            }
        }
    }
}

#[must_use]
pub struct ReentrantMutexGuard<'a, T> where T: 'static {
    mutex: &'a ReentrantMutex<T>,
}

impl<'a, T> Drop for ReentrantMutexGuard<'a, T> {
    fn drop(&mut self) {
        self.mutex.unlock()
    }
}

impl<'a, T> Deref for ReentrantMutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.mutex.data
    }
}
