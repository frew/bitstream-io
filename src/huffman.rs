// Copyright 2017 Brian Langenberger
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Traits and implementations for reading or writing Huffman codes
//! from or to a stream.

use std::fmt;
use std::collections::BTreeMap;

pub enum ReadHuffmanTree<T: Clone> {
    Leaf(T),
    Tree(Box<ReadHuffmanTree<T>>, Box<ReadHuffmanTree<T>>)
}

impl<T: Clone> ReadHuffmanTree<T> {
    /// Given a vector of symbol/code pairs, compiles a Huffman tree
    /// for reading.
    /// Code must be 0 or 1 bits and are always consumed from the stream
    /// from least-significant in the list to most signficant
    /// (which makes them easier to read for humans).
    ///
    /// Each code in the tree must be unique, but symbols may occur
    /// multiple times.  All possible codes must be assigned some symbol.
    ///
    /// ## Example 1
    /// ```
    /// use bitstream_io::huffman::ReadHuffmanTree;
    /// assert!(ReadHuffmanTree::new(vec![(1i32, vec![0]),
    ///                                   (2i32, vec![1, 0]),
    ///                                   (3i32, vec![1, 1])]).is_ok());
    /// ```
    ///
    /// ## Example 2
    /// Note how the `1 0` code has no symbol, so this tree cannot be
    /// built for reading.
    ///
    /// ```
    /// use bitstream_io::huffman::ReadHuffmanTree;
    /// assert!(ReadHuffmanTree::new(vec![(1i32, vec![0]),
    ///                                   (3i32, vec![1, 1])]).is_err());
    /// ```
    pub fn new(values: Vec<(T, Vec<u8>)>) ->
        Result<ReadHuffmanTree<T>,HuffmanTreeError> {
        let mut tree = WipHuffmanTree::new_empty();

        for (symbol, code) in values.into_iter() {
            tree.add(code.as_slice(), symbol)?;
        }

        tree.into_read_tree()
    }
}

// Work-in-progress trees may have empty nodes during construction
// but those are not allowed in a finalized tree.
// If the user wants some codes to be None or an error symbol of some sort,
// those will need to be specified explicitly.
enum WipHuffmanTree<T: Clone> {
    Empty,
    Leaf(T),
    Tree(Box<WipHuffmanTree<T>>, Box<WipHuffmanTree<T>>)
}

impl<T: Clone> WipHuffmanTree<T> {
    fn new_empty() -> WipHuffmanTree<T> {
        WipHuffmanTree::Empty
    }

    fn new_leaf(value: T) -> WipHuffmanTree<T> {
        WipHuffmanTree::Leaf(value)
    }

    fn new_tree() -> WipHuffmanTree<T> {
        WipHuffmanTree::Tree(Box::new(Self::new_empty()),
                             Box::new(Self::new_empty()))
    }

    fn into_read_tree(self) -> Result<ReadHuffmanTree<T>,HuffmanTreeError> {
        match self {
            WipHuffmanTree::Empty => {
                Err(HuffmanTreeError::MissingLeaf)
            }
            WipHuffmanTree::Leaf(v) => {
                Ok(ReadHuffmanTree::Leaf(v))
            }
            WipHuffmanTree::Tree(zero, one) => {
                let zero = zero.into_read_tree()?;
                let one = one.into_read_tree()?;
                Ok(ReadHuffmanTree::Tree(Box::new(zero), Box::new(one)))
            }
        }
    }

    fn add(&mut self, code: &[u8], symbol: T) -> Result<(),HuffmanTreeError> {
        match self {
            &mut WipHuffmanTree::Empty => {
                if code.len() == 0 {
                    *self = WipHuffmanTree::new_leaf(symbol);
                    Ok(())
                } else {
                    *self = WipHuffmanTree::new_tree();
                    self.add(code, symbol)
                }
            }
            &mut WipHuffmanTree::Leaf(_) => {
                Err(if code.len() == 0 {
                    HuffmanTreeError::DuplicateLeaf
                } else {
                    HuffmanTreeError::OrphanedLeaf
                })
            }
            &mut WipHuffmanTree::Tree(ref mut zero, ref mut one) => {
                if code.len() == 0 {
                    Err(HuffmanTreeError::DuplicateLeaf)
                } else {
                    match code[0] {
                        0 => {zero.add(&code[1..], symbol)}
                        1 => {one.add(&code[1..], symbol)}
                        _ => {Err(HuffmanTreeError::InvalidBit)}
                    }
                }
            }
        }
    }
}

#[derive(PartialEq, Copy, Clone, Debug)]
pub enum HuffmanTreeError {
    InvalidBit,
    MissingLeaf,
    DuplicateLeaf,
    OrphanedLeaf
}

impl fmt::Display for HuffmanTreeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            HuffmanTreeError::InvalidBit => {
                write!(f, "invalid bit in specification")
            }
            HuffmanTreeError::MissingLeaf => {
                write!(f, "missing leaf node in specification")
            }
            HuffmanTreeError::DuplicateLeaf => {
                write!(f, "duplicate leaf node in specification")
            }
            HuffmanTreeError::OrphanedLeaf => {
                write!(f, "orphaned leaf node in specification")
            }
        }
    }
}

pub struct WriteHuffmanTree<T: Ord> {
    big_endian: BTreeMap<T,(u32,u64)>,
    little_endian: BTreeMap<T,(u32,u64)>
}

impl<T: Ord + Clone> WriteHuffmanTree<T> {
    /// Given a vector of symbol/code pairs, compiles a Huffman tree
    /// for writing.
    /// Code must be 0 or 1 bits and are always written to the stream
    /// from least-significant in the list to most signficant
    /// (which makes them easier to read for humans).
    ///
    /// If the same symbol occurs multiple times, the first code is used.
    /// Unlike in read trees, not all possible codes need to be
    /// assigned a symbol.
    ///
    /// ## Example
    /// ```
    /// use bitstream_io::huffman::WriteHuffmanTree;
    /// assert!(WriteHuffmanTree::new(vec![(1i32, vec![0]),
    ///                                    (2i32, vec![1, 0]),
    ///                                    (3i32, vec![1, 1])]).is_ok());
    /// ```
    pub fn new(values: Vec<(T, Vec<u8>)>) ->
        Result<WriteHuffmanTree<T>,HuffmanTreeError> {
        use super::{BitQueueBE, BitQueueLE, BitQueue};

        // This current implementation is limited to Huffman codes
        // that generate up to 64 bits.  It may need to be updated
        // if I can find anything larger.

        let mut big_endian = BTreeMap::new();
        let mut little_endian = BTreeMap::new();

        for (symbol, code) in values.into_iter() {
            let mut be_encoded = BitQueueBE::new();
            let mut le_encoded = BitQueueLE::new();
            let code_len = code.len() as u32;
            for bit in code {
                if (bit != 0) && (bit != 1) {
                    return Err(HuffmanTreeError::InvalidBit);
                }
                be_encoded.push(1, bit as u64);
                le_encoded.push(1, bit as u64);
            }
            big_endian.entry(symbol.clone())
                      .or_insert((code_len, be_encoded.value()));
            little_endian.entry(symbol)
                         .or_insert((code_len, le_encoded.value()));
        }

        Ok(WriteHuffmanTree{big_endian: big_endian,
                            little_endian: little_endian})
    }

    /// Returns true if symbol is in tree.
    pub fn has_symbol(&self, symbol: T) -> bool {
        self.big_endian.contains_key(&symbol)
    }

    /// Given symbol, returns big-endian (bits, value) pair
    /// for writing code.  Panics if symbol is not found.
    pub fn get_be(&self, symbol: T) -> (u32, u64) {
        self.big_endian[&symbol]
    }

    /// Given symbol, returns little-endian (bits, value) pair
    /// for writing code.  Panics if symbol is not found.
    pub fn get_le(&self, symbol: T) -> (u32, u64) {
        self.little_endian[&symbol]
    }
}