#![cfg_attr(not(test), no_std)]

use core::cell::UnsafeCell;
use portable_atomic::{AtomicBool, AtomicU8, Ordering};

pub struct TripleBuffer<T> {
    buffers: [UnsafeCell<T>; 3],

    back_info: AtomicBackBufferInfo,
    input_idx: AtomicBackBufferInfo,
    output_idx: AtomicBackBufferInfo,

    is_reader_exist: AtomicFlag,
    is_writer_exist: AtomicFlag,
}

pub struct BufferReader<'a, T> {
    read_buffer: &'a TripleBuffer<T>,
}

pub struct BufferWriter<'a, T> {
    write_buffer: &'a TripleBuffer<T>,
}

impl<'a, T> BufferReader<'a, T> {
    pub fn read(&mut self) -> &T {
        self.update();
        self.output_buffer()
    }

    pub fn updated(&mut self) -> bool {
        let back_info = self.read_buffer.back_info.load(Ordering::Acquire);
        back_info & BACK_DIRTY_BIT != 0
    }

    pub fn output_buffer(&mut self) -> &mut T {
        let output_ptr = self.read_buffer.buffers
            [self.read_buffer.output_idx.load(Ordering::Acquire) as usize]
            .get();
        unsafe { &mut *output_ptr }
    }

    pub fn update(&mut self) -> bool {
        // let buffer_state = &(*self.buffer);
        let updated = self.updated();
        if updated {
            let former_back_info = self.read_buffer.back_info.swap(
                self.read_buffer.output_idx.load(Ordering::Acquire),
                Ordering::AcqRel,
            );
            self.read_buffer
                .output_idx
                .store(former_back_info & BACK_INDEX_MASK, Ordering::Release);
        }
        updated
    }
}

impl<'a, T> Drop for BufferReader<'a, T> {
    fn drop(&mut self) {
        self.read_buffer
            .is_reader_exist
            .store(false, Ordering::Release)
    }
}

impl<'a, T> BufferWriter<'a, T> {
    pub fn write(&mut self, value: T) {
        *self.input_buffer() = value;
        self.publish();
    }

    pub fn input_buffer(&mut self) -> &mut T {
        let input_ptr = self.write_buffer.buffers
            [self.write_buffer.input_idx.load(Ordering::Acquire) as usize]
            .get();
        unsafe { &mut *input_ptr }
    }

    pub fn consumed(&self) -> bool {
        let back_info = self.write_buffer.back_info.load(Ordering::Acquire);
        back_info & BACK_DIRTY_BIT == 0
    }

    pub fn publish(&self) -> bool {
        let former_back_info = self.write_buffer.back_info.swap(
            self.write_buffer.input_idx.load(Ordering::Acquire) | BACK_DIRTY_BIT,
            Ordering::AcqRel,
        );

        self.write_buffer
            .input_idx
            .store(former_back_info & BACK_INDEX_MASK, Ordering::Release);

        former_back_info & BACK_DIRTY_BIT != 0
    }
}

impl<'a, T> Drop for BufferWriter<'a, T> {
    fn drop(&mut self) {
        self.write_buffer
            .is_writer_exist
            .store(false, Ordering::Release)
    }
}

unsafe impl<T> Sync for TripleBuffer<T> {}

impl<T> TripleBuffer<T> {
    pub fn new(generator: impl Fn() -> T) -> Self {
        Self::new_const(generator(), generator(), generator())
    }

    pub const fn new_const(s1: T, s2: T, s3: T) -> Self {
        Self {
            buffers: [
                UnsafeCell::new(s1),
                UnsafeCell::new(s2),
                UnsafeCell::new(s3),
            ],
            back_info: AtomicBackBufferInfo::new(0),
            input_idx: AtomicBackBufferInfo::new(1),
            output_idx: AtomicBackBufferInfo::new(2),

            is_reader_exist: AtomicFlag::new(false),
            is_writer_exist: AtomicFlag::new(false),
        }
    }

    pub fn get_reader(&self) -> BufferReader<T> {
        loop {
            match self.is_reader_exist.compare_exchange(
                false,
                true,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return BufferReader { read_buffer: self },
                Err(false) => continue,
                Err(true) => panic!("Reader already exists"),
            }
        }
    }

    pub fn get_writer(&self) -> BufferWriter<T> {
        loop {
            match self.is_writer_exist.compare_exchange(
                false,
                true,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return BufferWriter { write_buffer: self },
                Err(false) => continue,
                Err(true) => panic!("Writer already exists"),
            }
        }
    }
}

type AtomicBackBufferInfo = AtomicU8;
type AtomicFlag = AtomicBool;

const BACK_INDEX_MASK: u8 = 0b11;
const BACK_DIRTY_BIT: u8 = 0b100;

#[cfg(test)]
mod tests {
    use std::ptr::addr_of;

    use super::*;
    use std::thread::Thread;

    #[derive(Default, PartialEq, Eq, Debug)]
    struct MyStruct {
        goose: u32,
    }

    #[test]
    fn my_test() {
        static goose_buffer: TripleBuffer<MyStruct> = TripleBuffer::<MyStruct>::new_const(
            MyStruct { goose: 0 },
            MyStruct { goose: 0 },
            MyStruct { goose: 0 },
        );
        let jh = std::thread::spawn(|| {
            let mut goose_writer = goose_buffer.get_writer();

            goose_writer.write(MyStruct { goose: 2 });
            goose_writer.write(MyStruct { goose: 3 });
            goose_writer.write(MyStruct { goose: 4 });
        });

        let mut goose_reader = goose_buffer.get_reader();
        let evil_goose_1 = goose_reader.read();
        let evil_goose_2 = goose_reader.read();
        let evil_goose_3 = goose_reader.read();

        println!("{:?}", *evil_goose_3);
        jh.join().unwrap();

        assert!(*goose_reader.read() == MyStruct { goose: 4 })
    }

    #[test]
    fn my_other_test() {
        static goose_buffer: TripleBuffer<MyStruct> = TripleBuffer::<MyStruct>::new_const(
            MyStruct { goose: 0 },
            MyStruct { goose: 0 },
            MyStruct { goose: 0 },
        );

        let count = 1000;

        let jh = std::thread::spawn(move || {
            let mut goose_writer = goose_buffer.get_writer();
            for i in 0..=count {
                goose_writer.write(MyStruct { goose: i as u32 });
            }
        });

        let mut goose_reader = goose_buffer.get_reader();
        for _ in 0..=count {
            goose_reader.read();
        }
        jh.join().unwrap();
        assert!(*goose_reader.read() == MyStruct { goose: count })
    }

    #[test]
    #[should_panic]
    fn reader_access_test() {
        static goose_buffer: TripleBuffer<MyStruct> = TripleBuffer::<MyStruct>::new_const(
            MyStruct { goose: 0 },
            MyStruct { goose: 0 },
            MyStruct { goose: 0 },
        );
        let mut goose_reader = goose_buffer.get_reader();
        let mut evil_reader = goose_buffer.get_reader();
    }

    #[test]
    fn good_reader_access_test() {
        static goose_buffer: TripleBuffer<MyStruct> = TripleBuffer::<MyStruct>::new_const(
            MyStruct { goose: 0 },
            MyStruct { goose: 0 },
            MyStruct { goose: 0 },
        );
        {
            let mut goose_reader = goose_buffer.get_reader();
        }
        let mut evil_reader = goose_buffer.get_reader();
    }

    #[test]
    fn direct_input_test() {
        #[derive(Default, PartialEq, Eq, Debug)]
        struct Cat {
            is_there_cat: bool,
        }

        impl Cat {
            fn bring_cat(&mut self) {
                self.is_there_cat = true;
            }
        }
        #[derive(Default, PartialEq, Eq, Debug)]
        struct MyBiggerStruct {
            goose: u32,
            duck: u32,
            cat: Cat,
        }
        static goose_buffer: TripleBuffer<MyBiggerStruct> =
            TripleBuffer::<MyBiggerStruct>::new_const(
                MyBiggerStruct {
                    goose: 0,
                    duck: 1,
                    cat: Cat {
                        is_there_cat: false,
                    },
                },
                MyBiggerStruct {
                    goose: 0,
                    duck: 1,
                    cat: Cat {
                        is_there_cat: false,
                    },
                },
                MyBiggerStruct {
                    goose: 0,
                    duck: 1,
                    cat: Cat {
                        is_there_cat: false,
                    },
                },
            );
        let jh = std::thread::spawn(|| {
            let mut goose_writer = goose_buffer.get_writer();

            let temp_goose = goose_writer.input_buffer();
            temp_goose.goose = 4;
            temp_goose.duck = 2;
            temp_goose.cat.bring_cat();
            goose_writer.publish()
        });

        let mut goose_reader = goose_buffer.get_reader();
        let evil_goose_1 = goose_reader.read();
        let evil_goose_2 = goose_reader.read();
        let evil_goose_3 = goose_reader.read();

        println!("{:?}", *evil_goose_3);
        jh.join().unwrap();

        assert!(
            *goose_reader.read()
                == MyBiggerStruct {
                    goose: 4,
                    duck: 2,
                    cat: Cat { is_there_cat: true }
                }
        )
    }
}
