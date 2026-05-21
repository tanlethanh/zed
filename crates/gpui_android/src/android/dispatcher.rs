use std::{
    thread,
    time::{Duration, Instant},
};

use gpui::{
    GLOBAL_THREAD_TIMINGS, PlatformDispatcher, Priority, PriorityQueueReceiver,
    PriorityQueueSender, RunnableVariant, THREAD_TIMINGS, TaskTiming, ThreadTaskTimings, profiler,
};

struct TimerAfter {
    duration: Duration,
    runnable: RunnableVariant,
}

pub(crate) struct AndroidDispatcher {
    main_sender: PriorityQueueSender<RunnableVariant>,
    timer_sender: std::sync::mpsc::Sender<TimerAfter>,
    background_sender: PriorityQueueSender<RunnableVariant>,
    _background_threads: Vec<thread::JoinHandle<()>>,
    main_thread_id: thread::ThreadId,
}

const MIN_THREADS: usize = 2;

impl AndroidDispatcher {
    pub fn new(main_sender: PriorityQueueSender<RunnableVariant>) -> Self {
        let (background_sender, background_receiver) = PriorityQueueReceiver::new();
        let thread_count =
            std::thread::available_parallelism().map_or(MIN_THREADS, |i| i.get().max(MIN_THREADS));

        let mut background_threads = (0..thread_count)
            .map(|i| {
                let receiver: PriorityQueueReceiver<RunnableVariant> = background_receiver.clone();
                std::thread::Builder::new()
                    .name(format!("Worker-{i}"))
                    .spawn(move || {
                        for runnable in receiver.iter() {
                            let start = Instant::now();

                            let location = runnable.metadata().location;
                            let mut timing = TaskTiming {
                                location,
                                start,
                                end: None,
                            };
                            profiler::add_task_timing(timing);

                            runnable.run();

                            let end = Instant::now();
                            timing.end = Some(end);
                            profiler::add_task_timing(timing);

                            log::trace!(
                                "background thread {}: ran runnable. took: {:?}",
                                i,
                                start.elapsed()
                            );
                        }
                    })
                    .unwrap()
            })
            .collect::<Vec<_>>();

        let (timer_sender, timer_receiver) = std::sync::mpsc::channel::<TimerAfter>();
        let timer_thread = std::thread::Builder::new()
            .name("Timer".to_owned())
            .spawn(move || {
                while let std::result::Result::Ok(timer) = timer_receiver.recv() {
                    std::thread::sleep(timer.duration);

                    let runnable = timer.runnable;

                    let start = Instant::now();
                    let location = runnable.metadata().location;
                    let mut timing = TaskTiming {
                        location,
                        start,
                        end: None,
                    };
                    profiler::add_task_timing(timing);

                    runnable.run();

                    let end = Instant::now();
                    timing.end = Some(end);
                    profiler::add_task_timing(timing);
                }
            })
            .unwrap();

        background_threads.push(timer_thread);

        Self {
            main_sender,
            timer_sender,
            background_sender,
            _background_threads: background_threads,
            main_thread_id: thread::current().id(),
        }
    }
}

impl PlatformDispatcher for AndroidDispatcher {
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
        thread::current().id() == self.main_thread_id
    }

    fn dispatch(&self, runnable: RunnableVariant, priority: Priority) {
        self.background_sender
            .send(priority, runnable)
            .unwrap_or_else(|_| panic!("blocking sender returned without value"));
    }

    fn dispatch_on_main_thread(&self, runnable: RunnableVariant, priority: Priority) {
        self.main_sender
            .send(priority, runnable)
            .unwrap_or_else(|runnable| {
                std::mem::forget(runnable);
            });
    }

    fn dispatch_after(&self, duration: Duration, runnable: RunnableVariant) {
        self.timer_sender
            .send(TimerAfter { duration, runnable })
            .ok();
    }

    fn spawn_realtime(&self, f: Box<dyn FnOnce() + Send>) {
        std::thread::Builder::new()
            .name("Realtime".to_owned())
            .spawn(move || {
                f();
            })
            .ok();
    }
}

/// Simple receiver wrapper for Android (no calloop dependency)
pub struct AndroidQueueReceiver<T> {
    receiver: PriorityQueueReceiver<T>,
}

impl<T> AndroidQueueReceiver<T> {
    pub fn new() -> (PriorityQueueSender<T>, Self) {
        let (tx, rx) = PriorityQueueReceiver::new();
        (tx, Self { receiver: rx })
    }

    pub fn try_recv(&mut self) -> Option<T> {
        self.receiver.try_pop().ok().flatten()
    }
}
