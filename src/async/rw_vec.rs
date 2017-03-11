//! A mutex-like lock which can be shared between threads and can interact
//! with OpenCL events.
//!

extern crate qutex;

use std::ops::{Deref, DerefMut};
use futures::{task, Future, Poll, Async};
use futures::sync::oneshot;
use ::{Event, Result as OclResult};
use async::{Error as AsyncError, Result as AsyncResult};
use standard;

pub use self::qutex::qutex::{Request, Guard, FutureGuard, Qutex};

// Allows access to the data contained within a lock just like a mutex guard.
pub struct RwGuard<T> {
    rw_vec: RwVec<T>,
}

impl<T> Deref for RwGuard<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Vec<T> {
        unsafe { &*self.rw_vec.as_ptr() }
    }
}

impl<T> DerefMut for RwGuard<T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        unsafe { &mut *self.rw_vec.as_mut_ptr() }
    }
}

impl<T> Drop for RwGuard<T> {
    fn drop(&mut self) {
        unsafe { self.rw_vec.unlock() };
    }
}


/// Like a `FutureGuard` but additionally waits on an OpenCL event.
pub struct PendingRwGuard<T> {
    rw_vec: Option<RwVec<T>>,
    rx: oneshot::Receiver<()>,
    wait_event: Event,
    trigger_event: Event,
    len: usize,
}

impl<T> PendingRwGuard<T> {
    fn new(rw_vec: RwVec<T>, rx: oneshot::Receiver<()>, wait_event: Event) -> OclResult<PendingRwGuard<T>> {
        let trigger_event = Event::user(&wait_event.context()?)?;
        let len = unsafe { (*rw_vec.as_ptr()).len() };

        Ok(PendingRwGuard {
            rw_vec: Some(rw_vec),
            rx: rx,
            wait_event: wait_event,
            trigger_event: trigger_event,
            len: len,
        })
    }

    pub fn trigger_event(&self) -> &Event {
        &self.trigger_event
    }

    pub fn wait(self) -> AsyncResult<RwGuard<T>> {
        <Self as Future>::wait(self)
    }

    pub unsafe fn as_mut_ptr(&self) -> Option<*mut T> {
        self.rw_vec.as_ref().map(|rw_vec| (*rw_vec.as_mut_ptr()).as_mut_ptr())
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

impl<T> Future for PendingRwGuard<T> {
    type Item = RwGuard<T>;
    type Error = AsyncError;

    #[inline]
    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        if self.rw_vec.is_some() {
            unsafe { self.rw_vec.as_ref().unwrap().process_queue(); }

            if !self.wait_event.is_complete()? {
                let task_ptr = standard::box_raw_void(task::park());
                    unsafe { self.wait_event.set_callback(standard::_unpark_task, task_ptr)?; };
                    return Ok(Async::NotReady);
            }

            match self.rx.poll() {
                Ok(status) => Ok(status.map(|_| {
                    RwGuard { rw_vec: self.rw_vec.take().unwrap() }
                })),
                Err(e) => return Err(e.into()),
            }
        } else {
            Err("PendingRwGuard::poll: Task already completed.".into())
        }
    }
}

#[derive(Clone)]
pub struct RwVec<T> {
    qutex: Qutex<Vec<T>>,
}

impl<T> RwVec<T> {
    /// Creates and returns a new `RwVec`.
    #[inline]
    pub fn new() -> RwVec<T> {
        RwVec {
            qutex: Qutex::new(Vec::new())
        }
    }

    pub fn lock_pending_event(&self, wait_event: Event) -> OclResult<PendingRwGuard<T>> {
        let (tx, rx) = oneshot::channel();
        unsafe { self.qutex.push_request(Request::new(tx)); }
        PendingRwGuard::new((*self).clone().into(), rx, wait_event)
    }
}

impl<T> From<Qutex<Vec<T>>> for RwVec<T> {
    fn from(q: Qutex<Vec<T>>) -> RwVec<T> {
        RwVec { qutex: q }
    }
}

impl<T> From<Vec<T>> for RwVec<T> {
    fn from(vec: Vec<T>) -> RwVec<T> {
        RwVec { qutex: Qutex::new(vec) }
    }
}

impl<T> Deref for RwVec<T> {
    type Target = Qutex<Vec<T>>;

    fn deref(&self) -> &Qutex<Vec<T>> {
        &self.qutex
    }
}

impl<T> DerefMut for RwVec<T> {
    fn deref_mut(&mut self) -> &mut Qutex<Vec<T>> {
        &mut self.qutex
    }
}