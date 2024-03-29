#![no_std]

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicUsize, Ordering},
};

pub struct RingBuffer<T, const N: usize> {
    /// the index of the first initialised element, plus k * N
    start: AtomicUsize,
    /// the index of the first uninitialised element, plus k * N
    end: AtomicUsize,
    /// the index of the first non-reserved slot, plus k * N
    reserved: AtomicUsize,
    data: [UnsafeCell<MaybeUninit<T>>; N],
}

unsafe impl<T, const N: usize> Send for RingBuffer<T, N> {}
unsafe impl<T, const N: usize> Sync for RingBuffer<T, N> {}

impl<T, const N: usize> RingBuffer<T, N> {
    pub const fn new() -> Self {
        RingBuffer {
            start: AtomicUsize::new(0),
            end: AtomicUsize::new(0),
            reserved: AtomicUsize::new(0),
            data: unsafe { MaybeUninit::uninit().assume_init() },
        }
    }

    pub fn try_insert(&self, v: T) -> Result<(), T> {
        let place = loop {
            let reserved = self.reserved.load(Ordering::Relaxed);
            let start = self.start.load(Ordering::Acquire);
            if reserved == start + N {
                return Err(v);
            }
            match self.reserved.compare_exchange_weak(
                reserved,
                reserved + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break reserved,
                Err(_) => {}
            }
        };
        let index = place % N;
        unsafe {
            self.data[index].get().write_volatile(MaybeUninit::new(v));
        }
        loop {
            match self.end.compare_exchange_weak(
                place,
                place + 1,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(_) => {}
            }
        }
        Ok(())
    }

    pub fn try_get(&self) -> Option<T> {
        loop {
            let start = self.start.load(Ordering::Relaxed);
            let end = self.end.load(Ordering::Acquire);
            if start == end {
                return None;
            }
            let start_index = start % N;
            let val_uninit = unsafe { self.data[start_index].get().read_volatile() };
            match self.start.compare_exchange_weak(
                start,
                start + 1,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => return unsafe { Some(val_uninit.assume_init()) },
                Err(_) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_thread_simple() {
        let queue = RingBuffer::<u32, 4>::new();
        assert!(queue.try_insert(1).is_ok());
        assert_eq!(queue.try_get(), Some(1));
    }

    #[test]
    fn single_thread_overflow() {
        let queue = RingBuffer::<u32, 4>::new();
        assert!(queue.try_insert(1).is_ok());
        assert!(queue.try_insert(2).is_ok());
        assert!(queue.try_insert(3).is_ok());

        assert_eq!(queue.try_get(), Some(1));
        assert_eq!(queue.try_get(), Some(2));
        assert_eq!(queue.try_get(), Some(3));

        assert!(queue.try_insert(4).is_ok());
        assert!(queue.try_insert(5).is_ok());

        assert_eq!(queue.try_get(), Some(4));
        assert_eq!(queue.try_get(), Some(5));
    }

    #[test]
    fn single_thread_full() {
        let queue = RingBuffer::<u32, 4>::new();
        assert!(queue.try_insert(1).is_ok());
        assert!(queue.try_insert(2).is_ok());
        assert!(queue.try_insert(3).is_ok());
        assert!(queue.try_insert(4).is_ok());
        assert!(queue.try_insert(5).is_err());
    }

    #[test]
    fn two_thread_simple() {
        let queue = RingBuffer::<u32, 4>::new();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                assert!(queue.try_insert(5).is_ok());
            });
            loop {
                if let Some(v) = queue.try_get() {
                    assert!(v == 5);
                    break;
                }
            }
        });
    }

    #[test]
    fn two_thread_overflow() {
        let queue = RingBuffer::<u32, 4>::new();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                assert!(queue.try_insert(1).is_ok());
                assert!(queue.try_insert(2).is_ok());
                assert!(queue.try_insert(3).is_ok());
                assert!(queue.try_insert(4).is_ok());
                while queue.try_insert(5).is_err() {}
            });
            let mut x = 1;
            loop {
                if x == 6 {
                    break;
                }
                if let Some(v) = queue.try_get() {
                    assert_eq!(v, x);
                    println!("received {}", v);
                    x += 1;
                }
            }
        });
    }

    #[test]
    fn two_thread_count_million() {
        let queue = RingBuffer::<u32, 16>::new();
        let n = 1_000_000;
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut x = 0;
                while x <= n {
                    while queue.try_insert(x).is_err() {}
                    x += 1;
                }
            });
            let mut x = 0;
            while x < n {
                if let Some(y) = queue.try_get() {
                    assert_eq!(y, x);
                    x += 1;
                }
            }
        });
    }

    #[test]
    fn two_producer_one_consumer() {
        let queue = RingBuffer::<u64, 32>::new();
        let n = 1_000_000;
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for i in 0..n / 2 {
                    while queue.try_insert(i * 2).is_err() {}
                }
            });
            scope.spawn(|| {
                for i in 0..n / 2 {
                    while queue.try_insert(i * 2 + 1).is_err() {}
                }
            });
            let mut x = 0;
            while x < (n - 1) * n / 2 {
                if let Some(y) = queue.try_get() {
                    x += y;
                }
            }
        });
    }
}
