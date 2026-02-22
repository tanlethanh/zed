#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use gpui::{
    GLOBAL_THREAD_TIMINGS, PlatformDispatcher, Priority, RunnableMeta, RunnableVariant,
    THREAD_TIMINGS, TaskTiming, ThreadTaskTimings,
};

use async_task::Runnable;
use objc::{
    class, msg_send,
    runtime::{BOOL, YES},
    sel, sel_impl,
};
use std::{
    ffi::c_void,
    ptr::{NonNull, addr_of},
    time::{Duration, Instant},
};

pub(crate) mod dispatch_sys {
    include!(concat!(env!("OUT_DIR"), "/dispatch_sys.rs"));
}

use dispatch_sys::*;
pub(crate) fn dispatch_get_main_queue() -> dispatch_queue_t {
    addr_of!(_dispatch_main_q) as *const _ as dispatch_queue_t
}

pub(crate) struct IosDispatcher;

impl IosDispatcher {
    pub fn new() -> Self {
        Self
    }
}

impl PlatformDispatcher for IosDispatcher {
    fn get_all_timings(&self) -> Vec<ThreadTaskTimings> {
        let global_timings = GLOBAL_THREAD_TIMINGS.lock();
        ThreadTaskTimings::convert(&global_timings)
    }

    fn get_current_thread_timings(&self) -> ThreadTaskTimings {
        THREAD_TIMINGS.with(|timings| {
            let timings = timings.lock();
            let thread_name = timings.thread_name.clone();
            let total_pushed = timings.total_pushed;
            let timings = &timings.timings;

            let mut vec = Vec::with_capacity(timings.len());

            let (s1, s2) = timings.as_slices();
            vec.extend_from_slice(s1);
            vec.extend_from_slice(s2);

            ThreadTaskTimings {
                thread_name,
                thread_id: std::thread::current().id(),
                timings: vec,
                total_pushed,
            }
        })
    }

    fn is_main_thread(&self) -> bool {
        let is_main_thread: BOOL = unsafe { msg_send![class!(NSThread), isMainThread] };
        is_main_thread == YES
    }

    fn dispatch(&self, runnable: RunnableVariant, priority: Priority) {
        let context = runnable.into_raw().as_ptr() as *mut c_void;

        let queue_priority = match priority {
            Priority::RealtimeAudio => {
                panic!("RealtimeAudio priority should use spawn_realtime, not dispatch")
            }
            Priority::High => DISPATCH_QUEUE_PRIORITY_HIGH as isize,
            Priority::Medium => DISPATCH_QUEUE_PRIORITY_DEFAULT as isize,
            Priority::Low => DISPATCH_QUEUE_PRIORITY_LOW as isize,
        };

        unsafe {
            dispatch_async_f(
                dispatch_get_global_queue(queue_priority, 0),
                context,
                Some(trampoline as unsafe extern "C" fn(*mut c_void)),
            );
        }
    }

    fn dispatch_on_main_thread(&self, runnable: RunnableVariant, _priority: Priority) {
        let context = runnable.into_raw().as_ptr() as *mut c_void;
        unsafe {
            dispatch_async_f(
                dispatch_get_main_queue(),
                context,
                Some(trampoline as unsafe extern "C" fn(*mut c_void)),
            );
        }
    }

    fn dispatch_after(&self, duration: Duration, runnable: RunnableVariant) {
        let context = runnable.into_raw().as_ptr() as *mut c_void;
        unsafe {
            let queue =
                dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_HIGH.try_into().unwrap(), 0);
            let when = dispatch_time(DISPATCH_TIME_NOW as u64, duration.as_nanos() as i64);
            dispatch_after_f(
                when,
                queue,
                context,
                Some(trampoline as unsafe extern "C" fn(*mut c_void)),
            );
        }
    }

    fn spawn_realtime(&self, f: Box<dyn FnOnce() + Send>) {
        std::thread::spawn(move || {
            f();
        });
    }
}

extern "C" fn trampoline(context: *mut c_void) {
    let runnable =
        unsafe { Runnable::<RunnableMeta>::from_raw(NonNull::new_unchecked(context as *mut ())) };

    let metadata = runnable.metadata();

    if metadata.is_closed() {
        return;
    }

    let location = metadata.location;

    let start = Instant::now();
    let timing = TaskTiming {
        location,
        start,
        end: None,
    };

    THREAD_TIMINGS.with(|timings| {
        let mut timings = timings.lock();
        let timings = &mut timings.timings;
        if let Some(last_timing) = timings.iter_mut().rev().next() {
            if last_timing.location == timing.location {
                return;
            }
        }

        timings.push_back(timing);
    });

    runnable.run();
    let end = Instant::now();

    THREAD_TIMINGS.with(|timings| {
        let mut timings = timings.lock();
        let timings = &mut timings.timings;
        let Some(last_timing) = timings.iter_mut().rev().next() else {
            return;
        };
        last_timing.end = Some(end);
    });
}
