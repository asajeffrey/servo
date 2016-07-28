/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! An implementation of re-entrant mutexes.
//!
//! Re-entrant mutexes are like mutexes, but where it is expected
//! that a single thread may own a lock more than once.

//! It provides the same interface as https://github.com/rust-lang/rust/blob/master/src/libstd/sys/common/remutex.rs
//! so if those types are ever exported, we should be able to replace this implemtation.

use core::nonzero::NonZero;
use std::cell::{Cell, UnsafeCell};
use std::mem;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LockResult, Mutex, MutexGuard, PoisonError, TryLockError, TryLockResult};

/// A type for thread ids.
// TODO: can we use the thread-id crate for this?

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct ThreadId(NonZero<usize>);

lazy_static!{ static ref THREAD_COUNT: AtomicUsize = AtomicUsize::new(1); }

impl ThreadId {
    #[allow(unsafe_code)]
    fn new() -> ThreadId {
        let number = THREAD_COUNT.fetch_add(1, Ordering::SeqCst);
        ThreadId(unsafe { NonZero::new(number) })
    }
    pub fn current() -> ThreadId {
        THREAD_ID.with(|tls| tls.clone())
    }
}

thread_local!{ static THREAD_ID: ThreadId = ThreadId::new() }

/// A type for atomic storage of thread ids.
#[derive(Debug)]
pub struct AtomicOptThreadId(AtomicUsize);

impl AtomicOptThreadId {
    pub fn new() -> AtomicOptThreadId {
        AtomicOptThreadId(AtomicUsize::new(0))
    }
    pub fn store(&self, value: Option<ThreadId>, ordering: Ordering) {
        let number = value.map(|id| *id.0).unwrap_or(0);
        self.0.store(number, ordering);
    }
    #[allow(unsafe_code)]
    pub fn load(&self, ordering: Ordering) -> Option<ThreadId> {
        let number = self.0.load(ordering);
        if number == 0 { None } else { Some(ThreadId(unsafe { NonZero::new(number) })) }
    }
    #[allow(unsafe_code)]
    pub fn swap(&self, value: Option<ThreadId>, ordering: Ordering) -> Option<ThreadId> {
        let number = value.map(|id| *id.0).unwrap_or(0);
        let number = self.0.swap(number, ordering);
        if number == 0 { None } else { Some(ThreadId(unsafe { NonZero::new(number) })) }
    }
}

/// A type for hand-over-hand mutexes.
///
/// These support `lock` and `unlock` functions. `lock` blocks waiting to become the
/// mutex owner. `unlock` can only be called by the lock owner, and panics otherwise.
/// They have the same happens-before and poisoning semantics as `Mutex`.

pub struct HandOverHandMutex {
    mutex: Mutex<()>,
    owner: AtomicOptThreadId,
    guard: UnsafeCell<Option<MutexGuard<'static, ()>>>,
}

impl HandOverHandMutex {
    pub fn new() -> HandOverHandMutex {
        HandOverHandMutex {
            mutex: Mutex::new(()),
            owner: AtomicOptThreadId::new(),
            guard: UnsafeCell::new(None),
        }
    }
    #[allow(unsafe_code)]
    pub fn lock(&self) -> LockResult<()> {
        match self.mutex.lock() {
            Ok(guard) => {
                unsafe { *self.guard.get().as_mut().unwrap() = mem::transmute(guard) };
                self.owner.store(Some(ThreadId::current()), Ordering::Relaxed);
                Ok(())
            },
            Err(_) => Err(PoisonError::new(())),
        }
    }
    #[allow(unsafe_code)]
    pub fn try_lock(&self) -> TryLockResult<()> {
        match self.mutex.try_lock() {
            Ok(guard) => {
                unsafe { *self.guard.get().as_mut().unwrap() = mem::transmute(guard) };
                self.owner.store(Some(ThreadId::current()), Ordering::Relaxed);
                Ok(())
            },
            Err(TryLockError::WouldBlock) => Err(TryLockError::WouldBlock),
            Err(TryLockError::Poisoned(_)) => Err(TryLockError::Poisoned(PoisonError::new(()))),
        }
    }
    #[allow(unsafe_code)]
    pub fn unlock(&self) {
        assert_eq!(Some(ThreadId::current()), self.owner.load(Ordering::Relaxed));
        self.owner.store(None, Ordering::Relaxed);
        unsafe { *self.guard.get().as_mut().unwrap() = None; }
    }
    pub fn owner(&self) -> Option<ThreadId> {
        self.owner.load(Ordering::Relaxed)
    }
}

#[allow(unsafe_code)]
unsafe impl Send for HandOverHandMutex {}

/// A type for re-entrant mutexes.
///
/// It provides the same interface as https://github.com/rust-lang/rust/blob/master/src/libstd/sys/common/remutex.rs

pub struct ReentrantMutex<T> {
    mutex: HandOverHandMutex,
    count: Cell<usize>,
    data: T,
}

#[allow(unsafe_code)]
unsafe impl<T> Sync for ReentrantMutex<T> where T: Send {}

impl<T> ReentrantMutex<T> {
    pub fn new(data: T) -> ReentrantMutex<T> {
        ReentrantMutex {
            mutex: HandOverHandMutex::new(),
            count: Cell::new(0),
            data: data,
        }
    }

    pub fn lock(&self) -> LockResult<ReentrantMutexGuard<T>> {
        let result = ReentrantMutexGuard { mutex: self };
        if self.mutex.owner() != Some(ThreadId::current()) {
            if let Err(_) = self.mutex.lock() {
                return Err(PoisonError::new(result));
            }
        }
        self.count.set(self.count.get().checked_add(1).expect("Overflowed lock count."));
        Ok(result)
    }

    pub fn try_lock(&self) -> TryLockResult<ReentrantMutexGuard<T>> {
        let result = ReentrantMutexGuard { mutex: self };
        if self.mutex.owner() != Some(ThreadId::current()) {
            if let Err(err) = self.mutex.try_lock() {
                match err {
                    TryLockError::WouldBlock => return Err(TryLockError::WouldBlock),
                    TryLockError::Poisoned(_) => return Err(TryLockError::Poisoned(PoisonError::new(result))),
                }
            }
        }
        self.count.set(self.count.get().checked_add(1).expect("Overflowed lock count."));
        Ok(result)
    }

    fn unlock(&self) {
        let count = self.count.get().checked_sub(1).expect("Underflowed lock count.");
        self.count.set(count);
        if count == 0 {
            self.mutex.unlock();
        }
    }
}

#[must_use]
pub struct ReentrantMutexGuard<'a, T> where T: 'static {
    mutex: &'a ReentrantMutex<T>,
}

impl<'a, T> Drop for ReentrantMutexGuard<'a, T> {
    #[allow(unsafe_code)]
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
