use std::{
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
    pub fn new() -> Self {
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
            let start = self.start.load(Ordering::Relaxed);
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
            let end = self.end.load(Ordering::Relaxed);
            if end != place {
                continue;
            }
            // the buffer ends just at the point we have written to - good
            // this check maintains that everything between start and end is initialised
            let end_next = end + 1;
            match self.end.compare_exchange_weak(
                end,
                end_next,
                Ordering::Relaxed,
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
            let end = self.end.load(Ordering::Relaxed);
            if start == end {
                return None;
            }
            let start_index = start % N;
            let val_uninit = unsafe { self.data[start_index].get().read_volatile() };
            match self.start.compare_exchange_weak(
                start,
                start + 1,
                Ordering::Relaxed,
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
}
