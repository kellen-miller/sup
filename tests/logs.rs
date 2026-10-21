#![cfg(test)]

use std::collections::HashSet;
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn concurrent_first_runs_create_shared_cache_parents_safely() {
    let home = tempfile::tempdir().unwrap();
    let barrier = Arc::new(Barrier::new(24));
    let workers: Vec<_> = (0..24)
        .map(|_| {
            let home = home.path().to_path_buf();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                sup::logs::create_run_dir(&home).unwrap()
            })
        })
        .collect();
    let paths: HashSet<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
    assert_eq!(paths.len(), 24);
    assert!(paths.iter().all(|path| path.is_dir()));
}
