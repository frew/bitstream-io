// Copyright 2017 Brian Langenberger
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Traits and implementations for writing bits to a stream.
//!
//! ## Example
//!
//! Writing the initial STREAMINFO block to a FLAC file,
//! as documented in its
//! [specification](https://xiph.org/flac/format.html#stream).
//!
//! ```
//! use std::io::Write;
//! use bitstream_io::{BE, BitWriter};
//!
//! let mut flac: Vec<u8> = Vec::new();
//! {
//!     let mut writer = BitWriter::<BE>::new(&mut flac);
//!
//!     // stream marker
//!     writer.write_bytes(b"fLaC").unwrap();
//!
//!     // metadata block header
//!     let last_block: bool = false;
//!     let block_type: u8 = 0;
//!     let block_size: u32 = 34;
//!     writer.write_bit(last_block).unwrap();
//!     writer.write(7, block_type).unwrap();
//!     writer.write(24, block_size).unwrap();
//!
//!     // STREAMINFO block
//!     let minimum_block_size: u16 = 4096;
//!     let maximum_block_size: u16 = 4096;
//!     let minimum_frame_size: u32 = 1542;
//!     let maximum_frame_size: u32 = 8546;
//!     let sample_rate: u32 = 44100;
//!     let channels: u8 = 2;
//!     let bits_per_sample: u8 = 16;
//!     let total_samples: u64 = 304844;
//!     writer.write(16, minimum_block_size).unwrap();
//!     writer.write(16, maximum_block_size).unwrap();
//!     writer.write(24, minimum_frame_size).unwrap();
//!     writer.write(24, maximum_frame_size).unwrap();
//!     writer.write(20, sample_rate).unwrap();
//!     writer.write(3, channels - 1).unwrap();
//!     writer.write(5, bits_per_sample - 1).unwrap();
//!     writer.write(36, total_samples).unwrap();
//! }
//!
//! // STREAMINFO's MD5 sum
//!
//! // Note that the wrapped writer can be used once bitstream writing
//! // is finished at exactly the position one would expect.
//!
//! flac.write_all(
//!     b"\xFA\xF2\x69\x2F\xFD\xEC\x2D\x5B\x30\x01\x76\xB4\x62\x88\x7D\x92")
//!     .unwrap();
//!
//! assert_eq!(flac, vec![0x66,0x4C,0x61,0x43,0x00,0x00,0x00,0x22,
//!                       0x10,0x00,0x10,0x00,0x00,0x06,0x06,0x00,
//!                       0x21,0x62,0x0A,0xC4,0x42,0xF0,0x00,0x04,
//!                       0xA6,0xCC,0xFA,0xF2,0x69,0x2F,0xFD,0xEC,
//!                       0x2D,0x5B,0x30,0x01,0x76,0xB4,0x62,0x88,
//!                       0x7D,0x92]);
//! ```

#![warn(missing_docs)]

use std::io;

use super::{Numeric, SignedNumeric, BitQueue,
            Endianness, BigEndian, LittleEndian};
use huffman::WriteHuffmanTree;

/// For writing bit values to an underlying stream in a given endianness.
///
/// Because this only writes whole bytes to the underlying stream,
/// it is important that output is byte-aligned before the bitstream
/// writer's lifetime ends.
/// **Partial bytes will be lost** if the writer is disposed of
/// before they can be written.
pub struct BitWriter<'a, E: Endianness> {
    writer: &'a mut io::Write,
    bitqueue: BitQueue<E,u8>
}

impl<'a, E: Endianness> BitWriter<'a, E> {
    /// Wraps a BitWriter around something that implements `Write`
    pub fn new(writer: &mut io::Write) -> BitWriter<E> {
        BitWriter{writer: writer, bitqueue: BitQueue::new()}
    }

    /// Writes a single bit to the stream.
    /// `true` indicates 1, `false` indicates 0
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Examples
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{BigEndian, BitWriter};
    /// let mut data = Vec::new();
    /// {
    ///     let mut writer = BitWriter::<BigEndian>::new(&mut data);
    ///     writer.write_bit(true).unwrap();
    ///     writer.write_bit(false).unwrap();
    ///     writer.write_bit(true).unwrap();
    ///     writer.write_bit(true).unwrap();
    ///     writer.write_bit(false).unwrap();
    ///     writer.write_bit(true).unwrap();
    ///     writer.write_bit(true).unwrap();
    ///     writer.write_bit(true).unwrap();
    /// }
    /// assert_eq!(data, [0b10110111]);
    /// ```
    ///
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{LittleEndian, BitWriter};
    /// let mut data = Vec::new();
    /// {
    ///     let mut writer = BitWriter::<LittleEndian>::new(&mut data);
    ///     writer.write_bit(true).unwrap();
    ///     writer.write_bit(true).unwrap();
    ///     writer.write_bit(true).unwrap();
    ///     writer.write_bit(false).unwrap();
    ///     writer.write_bit(true).unwrap();
    ///     writer.write_bit(true).unwrap();
    ///     writer.write_bit(false).unwrap();
    ///     writer.write_bit(true).unwrap();
    /// }
    /// assert_eq!(data, [0b10110111]);
    /// ```
    pub fn write_bit(&mut self, bit: bool) -> Result<(), io::Error> {
        self.bitqueue.push(1, if bit {1} else {0});
        if self.bitqueue.is_full() {
            write_byte(self.writer, self.bitqueue.pop(8))
        } else {
            Ok(())
        }
    }

    /// Writes an unsigned value to the stream using the given
    /// number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if the input type is too small
    /// to hold the given number of bits.
    /// Returns an error if the value is too large
    /// to fit the given number of bits.
    ///
    /// # Examples
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{BigEndian, BitWriter};
    /// let mut data = Vec::new();
    /// {
    ///     let mut writer = BitWriter::<BigEndian>::new(&mut data);
    ///     writer.write(1, 0b1).unwrap();
    ///     writer.write(2, 0b01).unwrap();
    ///     writer.write(5, 0b10111).unwrap();
    /// }
    /// assert_eq!(data, [0b10110111]);
    /// ```
    ///
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{LittleEndian, BitWriter};
    /// let mut data = Vec::new();
    /// {
    ///     let mut writer = BitWriter::<LittleEndian>::new(&mut data);
    ///     writer.write(1, 0b1).unwrap();
    ///     writer.write(2, 0b11).unwrap();
    ///     writer.write(5, 0b10110).unwrap();
    /// }
    /// assert_eq!(data, [0b10110111]);
    /// ```
    ///
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{BigEndian, BitWriter};
    /// let mut data = Vec::new();
    /// let mut w = BitWriter::<BigEndian>::new(&mut data);
    /// assert!(w.write(9, 0u8).is_err());    // can't write  u8 in 9 bits
    /// assert!(w.write(17, 0u16).is_err());  // can't write u16 in 17 bits
    /// assert!(w.write(33, 0u32).is_err());  // can't write u32 in 33 bits
    /// assert!(w.write(65, 0u64).is_err());  // can't write u64 in 65 bits
    /// assert!(w.write(1, 2).is_err());      // can't write   2 in 1 bit
    /// assert!(w.write(2, 4).is_err());      // can't write   4 in 2 bits
    /// assert!(w.write(3, 8).is_err());      // can't write   8 in 3 bits
    /// assert!(w.write(4, 16).is_err());     // can't write  16 in 4 bits
    /// ```
    pub fn write<U>(&mut self, bits: u32, value: U) -> Result<(), io::Error>
        where U: Numeric {

        if bits > U::bits_size() {
            Err(io::Error::new(io::ErrorKind::InvalidInput,
                               "excessive bits for type written"))
        } else if (bits < U::bits_size()) && (value >= (U::one() << bits)) {
            Err(io::Error::new(io::ErrorKind::InvalidInput,
                               "excessive value for bits written"))
        } else if bits < self.bitqueue.remaining_len() {
            Ok(self.bitqueue.push(bits, value.to_u8()))
        } else {
            let mut acc = BitQueue::from_value(value, bits);
            write_unaligned(&mut self.writer, &mut acc, &mut self.bitqueue)
            .and_then(|()|
                write_aligned(&mut self.writer, &mut acc))
            .and_then(|()|
                Ok(self.bitqueue.push(acc.len(), acc.value().to_u8())))
        }
    }

    /// Writes the entirety of a byte buffer to the stream.
    /// If the stream is already byte-aligned, it will often
    /// map to a faster `write_all` call.  Otherwise it will
    /// write bytes individually in 8-bit increments.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Example
    ///
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{BigEndian, BitWriter};
    /// let mut data = Vec::new();
    /// {
    ///     let mut writer = BitWriter::<BigEndian>::new(&mut data);
    ///     writer.write(8, 0x66).unwrap();
    ///     writer.write(8, 0x6F).unwrap();
    ///     writer.write(8, 0x6F).unwrap();
    ///     writer.write_bytes(b"bar").unwrap();
    /// }
    /// assert_eq!(data, b"foobar");
    /// ```
    pub fn write_bytes(&mut self, buf: &[u8]) -> Result<(), io::Error> {
        if self.byte_aligned() {
            self.writer.write_all(buf)
        } else {
            for b in buf {
                self.write(8, *b)?;
            }
            Ok(())
        }
    }

    /// Writes Huffman code for the given symbol to the stream.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Example
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{BigEndian, BitWriter};
    /// use bitstream_io::huffman::compile_write_tree;
    /// let tree = compile_write_tree(
    ///     vec![('a', vec![0]),
    ///          ('b', vec![1, 0]),
    ///          ('c', vec![1, 1, 0]),
    ///          ('d', vec![1, 1, 1])]).unwrap();
    /// let mut data = Vec::new();
    /// {
    ///     let mut writer = BitWriter::<BigEndian>::new(&mut data);
    ///     writer.write_huffman(&tree, 'b').unwrap();
    ///     writer.write_huffman(&tree, 'c').unwrap();
    ///     writer.write_huffman(&tree, 'd').unwrap();
    /// }
    /// assert_eq!(data, [0b10110111]);
    /// ```
    pub fn write_huffman<T>(&mut self,
                            tree: &WriteHuffmanTree<E,T>,
                            symbol: T) ->
        Result<(), io::Error> where T: Ord + Copy {

        for &(bits, value) in tree.get(symbol) {
            self.write(bits, value)?;
        }
        Ok(())
    }

    /// Writes `value` number of 1 bits to the stream
    /// and then writes a 0 bit.  This field is variably-sized.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underyling stream.
    ///
    /// # Examples
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{BigEndian, BitWriter};
    /// let mut data = Vec::new();
    /// {
    ///     let mut writer = BitWriter::<BigEndian>::new(&mut data);
    ///     writer.write_unary0(0).unwrap();
    ///     writer.write_unary0(3).unwrap();
    ///     writer.write_unary0(10).unwrap();
    /// }
    /// assert_eq!(data, [0b01110111, 0b11111110]);
    /// ```
    ///
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{LittleEndian, BitWriter};
    /// let mut data = Vec::new();
    /// {
    ///     let mut writer = BitWriter::<LittleEndian>::new(&mut data);
    ///     writer.write_unary0(0).unwrap();
    ///     writer.write_unary0(3).unwrap();
    ///     writer.write_unary0(10).unwrap();
    /// }
    /// assert_eq!(data, [0b11101110, 0b01111111]);
    /// ```
    pub fn write_unary0(&mut self, value: u32) -> Result<(), io::Error> {
        match value {
            0 => {self.write_bit(false)}
            bits @ 1...31 => {self.write(value, (1u32 << bits) - 1)
                                  .and_then(|()| self.write_bit(false))}
            32 => {self.write(value, 0xFFFFFFFFu32)
                       .and_then(|()| self.write_bit(false))}
            bits @ 32...63  => {self.write(value, (1u64 << bits) - 1)
                                    .and_then(|()| self.write_bit(false))}
            64 => {self.write(value, 0xFFFFFFFFFFFFFFFFu64)
                       .and_then(|()| self.write_bit(false))}
            mut bits => {while bits > 64 {
                             self.write(64, 0xFFFFFFFFFFFFFFFFu64)?;
                             bits -= 64;
                         }
                         self.write_unary0(bits)}
        }
    }

    /// Writes `value` number of 0 bits to the stream
    /// and then writes a 1 bit.  This field is variably-sized.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underyling stream.
    ///
    /// # Example
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{BigEndian, BitWriter};
    /// let mut data = Vec::new();
    /// {
    ///     let mut writer = BitWriter::<BigEndian>::new(&mut data);
    ///     writer.write_unary1(0).unwrap();
    ///     writer.write_unary1(3).unwrap();
    ///     writer.write_unary1(10).unwrap();
    /// }
    /// assert_eq!(data, [0b10001000, 0b00000001]);
    /// ```
    ///
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{LittleEndian, BitWriter};
    /// let mut data = Vec::new();
    /// {
    ///     let mut writer = BitWriter::<LittleEndian>::new(&mut data);
    ///     writer.write_unary1(0).unwrap();
    ///     writer.write_unary1(3).unwrap();
    ///     writer.write_unary1(10).unwrap();
    /// }
    /// assert_eq!(data, [0b00010001, 0b10000000]);
    /// ```
    pub fn write_unary1(&mut self, value: u32) -> Result<(), io::Error> {
        match value {
            0        => {self.write_bit(true)}
            1...32   => {self.write(value, 0u32)
                             .and_then(|()| self.write_bit(true))}
            33...64  => {self.write(value, 0u64)
                             .and_then(|()| self.write_bit(true))}
            mut bits => {while bits > 64 {self.write(64, 0u64)?; bits -= 64;}
                         self.write_unary1(bits)}
        }
    }

    /// Returns true if the stream is aligned at a whole byte.
    ///
    /// # Example
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{BigEndian, BitWriter};
    /// let mut data = Vec::new();
    /// let mut writer = BitWriter::<BigEndian>::new(&mut data);
    /// assert_eq!(writer.byte_aligned(), true);
    /// writer.write(1, 0).unwrap();
    /// assert_eq!(writer.byte_aligned(), false);
    /// writer.write(7, 0).unwrap();
    /// assert_eq!(writer.byte_aligned(), true);
    /// ```
    #[inline(always)]
    pub fn byte_aligned(&self) -> bool {self.bitqueue.is_empty()}

    /// Pads the stream with 0 bits until it is aligned at a whole byte.
    /// Does nothing if the stream is already aligned.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underyling stream.
    ///
    /// # Example
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{BigEndian, BitWriter};
    /// let mut data = Vec::new();
    /// {
    ///     let mut writer = BitWriter::<BigEndian>::new(&mut data);
    ///     writer.write(1, 0).unwrap();
    ///     writer.byte_align().unwrap();
    ///     writer.write(8, 0xFF).unwrap();
    /// }
    /// assert_eq!(data, [0x00, 0xFF]);
    /// ```
    pub fn byte_align(&mut self) -> Result<(), io::Error> {
        while !self.byte_aligned() {
            self.write_bit(false)?;
        }
        Ok(())
    }

    /// Consumes writer and returns any un-written partial byte
    /// as a `(bits, value)` tuple.
    ///
    /// # Examples
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{BigEndian, BitWriter};
    /// let mut data = Vec::new();
    /// let (bits, value) = {
    ///     let mut writer = BitWriter::<BigEndian>::new(&mut data);
    ///     writer.write(15, 0b1010_0101_0101_101).unwrap();
    ///     writer.into_unwritten()
    /// };
    /// assert_eq!(data, [0b1010_0101]);
    /// assert_eq!(bits, 7);
    /// assert_eq!(value, 0b0101_101);
    /// ```
    ///
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{BigEndian, BitWriter};
    /// let mut data = Vec::new();
    /// let (bits, value) = {
    ///     let mut writer = BitWriter::<BigEndian>::new(&mut data);
    ///     writer.write(8, 0b1010_0101).unwrap();
    ///     writer.into_unwritten()
    /// };
    /// assert_eq!(data, [0b1010_0101]);
    /// assert_eq!(bits, 0);
    /// assert_eq!(value, 0);
    /// ```
    #[inline(always)]
    pub fn into_unwritten(self) -> (u32, u8) {
        (self.bitqueue.len(), self.bitqueue.value())
    }
}

impl<'a> BitWriter<'a, BigEndian> {
    /// Writes a twos-complement signed value to the stream
    /// with the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if the input type is too small
    /// to hold the given number of bits.
    /// Returns an error if the value is too large
    /// to fit the given number of bits.
    ///
    /// # Example
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{BigEndian, BitWriter};
    /// let mut data = Vec::new();
    /// {
    ///     let mut writer = BitWriter::<BigEndian>::new(&mut data);
    ///     writer.write_signed(4, -5).unwrap();
    ///     writer.write_signed(4, 7).unwrap();
    /// }
    /// assert_eq!(data, [0b10110111]);
    /// ```
    pub fn write_signed<S>(&mut self, bits: u32, value: S) ->
        Result<(), io::Error> where S: SignedNumeric {

        if bits > S::bits_size() {
            Err(io::Error::new(io::ErrorKind::InvalidInput,
                               "excessive bits for type written"))
        } else if value.is_negative() {
            self.write_bit(true)
            .and_then(|()| self.write(bits - 1, value.as_unsigned(bits)))
        } else {
            self.write_bit(false)
            .and_then(|()| self.write(bits - 1, value))
        }
    }

}

impl<'a> BitWriter<'a, LittleEndian> {
    /// Writes a twos-complement signed value to the stream
    /// with the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if the input type is too small
    /// to hold the given number of bits.
    /// Returns an error if the value is too large
    /// to fit the given number of bits.
    ///
    /// # Example
    /// ```
    /// use std::io::Write;
    /// use bitstream_io::{LittleEndian, BitWriter};
    /// let mut data = Vec::new();
    /// {
    ///     let mut writer = BitWriter::<LittleEndian>::new(&mut data);
    ///     writer.write_signed(4, 7).unwrap();
    ///     writer.write_signed(4, -5).unwrap();
    /// }
    /// assert_eq!(data, [0b10110111]);
    /// ```
    pub fn write_signed<S>(&mut self, bits: u32, value: S) ->
        Result<(), io::Error> where S: SignedNumeric {

        if bits > S::bits_size() {
            Err(io::Error::new(io::ErrorKind::InvalidInput,
                               "excessive bits for type written"))
        } else if value.is_negative() {
            self.write(bits - 1, value.as_unsigned(bits))
            .and_then(|()| self.write_bit(true))
        } else {
            self.write(bits - 1, value)
            .and_then(|()| self.write_bit(false))
        }
    }

}

#[inline]
fn write_byte(writer: &mut io::Write, byte: u8) -> Result<(),io::Error> {
    let buf = [byte];
    writer.write_all(&buf)
}

fn write_unaligned<E,N>(writer: &mut io::Write,
                        acc: &mut BitQueue<E,N>,
                        rem: &mut BitQueue<E,u8>) -> Result<(), io::Error>
    where E:Endianness, N: Numeric {

    if rem.is_empty() {
        Ok(())
    } else {
        use std::cmp::min;
        let bits_to_transfer = min(8 - rem.len(), acc.len());
        rem.push(bits_to_transfer, acc.pop(bits_to_transfer).to_u8());
        if rem.len() == 8 {
            write_byte(writer, rem.pop(8))
        } else {
            Ok(())
        }
    }
}

fn write_aligned<E,N>(writer: &mut io::Write,
                      acc: &mut BitQueue<E,N>) -> Result<(), io::Error>
    where E: Endianness, N: Numeric {

    let to_write = (acc.len() / 8) as usize;
    if to_write > 0 {
        // 64-bit types are the maximum supported
        debug_assert!(to_write <= 8);
        let mut buf = [0; 8];
        for b in buf[0..to_write].iter_mut() {
            *b = acc.pop(8).to_u8();
        }
        writer.write_all(&buf[0..to_write])
    } else {
        Ok(())
    }
}
