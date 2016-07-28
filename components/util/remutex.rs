/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! An implementation of re-entrant mutexes.
//!
//! Re-entrant mutexes are like mutexes, but where it is expected
//! that a single thread may own a lock more than once.

//! It provides the same interface as https://github.com/rust-lang/rust/blob/master/src/libstd/sys/common/remutex.rs
//! so if those types are ever exported, we should be able to replace this implemtation.

use std::cell::UnsafeCell;
use std::mem;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LockResult, Mutex, MutexGuard, PoisonError, TryLockError, TryLockResult};

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct ThreadId(usize);

lazy_static!{ static ref THREAD_COUNT: AtomicUsize = AtomicUsize::new(1); }

impl ThreadId {
    fn new() -> ThreadId {
        ThreadId(THREAD_COUNT.fetch_add(1, Ordering::SeqCst))
    }
    pub fn current() -> ThreadId {
        THREAD_ID.with(|tls| tls.clone())
    }
}

thread_local!{ static THREAD_ID: ThreadId = ThreadId::new() }

#[derive(Debug)]
pub struct AtomicOptThreadId(AtomicUsize);

impl AtomicOptThreadId {
    pub fn new() -> AtomicOptThreadId {
        AtomicOptThreadId(AtomicUsize::new(0))
    }
    pub fn store(&self, value: Option<ThreadId>, ordering: Ordering) {
        let number = value.map(|id| id.0).unwrap_or(0);
        self.0.store(number, ordering);
    }
    pub fn load(&self, ordering: Ordering) -> Option<ThreadId> {
        let number = self.0.load(ordering);
        if number == 0 { None } else { Some(ThreadId(number)) }
    }
    pub fn swap(&self, value: Option<ThreadId>, ordering: Ordering) -> Option<ThreadId> {
        let number = value.map(|id| id.0).unwrap_or(0);
        let number = self.0.swap(number, ordering);
        if number == 0 { None } else { Some(ThreadId(number)) }
    }
}

pub struct HandOverHandMutex {
    mutex: Mutex<()>,
    owner: AtomicOptThreadId,
    guard: UnsafeCell<Option<MutexGuard<'static, ()>>>,
}

#[allow(unsafe_code)]
impl HandOverHandMutex {
    pub fn new() -> HandOverHandMutex {
        HandOverHandMutex {
            mutex: Mutex::new(()),
            owner: AtomicOptThreadId::new(),
            guard: UnsafeCell::new(None),
        }
    }
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

pub struct ReentrantMutex<T> {
    mutex: HandOverHandMutex,
    count: AtomicUsize,
    data: T,
}

#[allow(unsafe_code)]
unsafe impl<T> Sync for ReentrantMutex<T> where T: Send {}

impl<T> ReentrantMutex<T> {
    pub fn new(data: T) -> ReentrantMutex<T> {
        ReentrantMutex {
            mutex: HandOverHandMutex::new(),
            count: AtomicUsize::new(0),
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
        self.count.fetch_add(1, Ordering::Relaxed);
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
        self.count.fetch_add(1, Ordering::Relaxed);
        Ok(result)
    }

    fn unlock(&self) {
        let count = self.count.fetch_sub(1, Ordering::Relaxed);
        if count <= 1 {
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
