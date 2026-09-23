//! One worker, one pending job, one latest result. New requests replace stale
//! pending work instead of spawning another thread for each track change.
use std::sync::{Arc, Condvar, Mutex};

struct State<J, R> {
    job: Option<J>,
    result: Option<R>,
    stopped: bool,
}
pub struct LatestWorker<J, R> {
    state: Arc<(Mutex<State<J, R>>, Condvar)>,
}
impl<J: Send + 'static, R: Send + 'static> LatestWorker<J, R> {
    pub fn new(mut work: impl FnMut(J) -> R + Send + 'static) -> Self {
        let state = Arc::new((
            Mutex::new(State {
                job: None,
                result: None,
                stopped: false,
            }),
            Condvar::new(),
        ));
        let shared = state.clone();
        std::thread::spawn(move || {
            loop {
                let (lock, wake) = &*shared;
                let mut state = lock.lock().unwrap();
                while state.job.is_none() && !state.stopped {
                    state = wake.wait(state).unwrap();
                }
                if state.stopped {
                    break;
                }
                let job = state.job.take().unwrap();
                drop(state);
                let result = work(job);
                lock.lock().unwrap().result = Some(result);
            }
        });
        Self { state }
    }
    pub fn submit(&self, job: J) {
        let (lock, wake) = &*self.state;
        lock.lock().unwrap().job = Some(job);
        wake.notify_one();
    }
    pub fn take(&self) -> Option<R> {
        self.state.0.lock().unwrap().result.take()
    }
}
impl<J, R> Drop for LatestWorker<J, R> {
    fn drop(&mut self) {
        let (lock, wake) = &*self.state;
        lock.lock().unwrap().stopped = true;
        wake.notify_one();
    }
}

pub fn cache_insert<T>(cache: &mut std::collections::HashMap<String, T>, key: String, value: T) {
    if !cache.contains_key(&key)
        && cache.len() >= 128
        && let Some(old) = cache.keys().next().cloned()
    {
        cache.remove(&old);
    }
    cache.insert(key, value);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_session_cache_is_bounded() {
        let mut cache = std::collections::HashMap::new();
        for i in 0..10000 {
            cache_insert(&mut cache, i.to_string(), i);
        }
        assert_eq!(cache.len(), 128);
        assert_eq!(cache["9999"], 9999);
    }
    #[test]
    fn requests_replace_pending_work() {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = LatestWorker::new(move |n| {
            if n == 0 {
                ready_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }
            n
        });
        worker.submit(0);
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        for n in 1..10000 {
            worker.submit(n);
        }
        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if worker.take() == Some(9999) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}
