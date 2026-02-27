use core::borrow;
use pathmap::morphisms::Catamorphism;
use pathmap::zipper::{
    ReadZipperTracked, ReadZipperUntracked, Zipper, ZipperAbsolutePath, ZipperForking,
    ZipperIteration, ZipperMoving, ZipperValues,
};
use pathmap::PathMap;
use rand::Rng;
use std::cell::{Ref, RefCell};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex, OnceLock};

use weighted_atom_sweep::{
    AtomHeader, AtomPosition, TraversalEngine, TraversalError, WeightedAtomSweep,
    WeightedAtomSweepSettings,
};

pub static GLOBAL_WS_SWEEP: Mutex<Option<WeightedAtomSweep<U64AtomHeader>>> = Mutex::new(None);

pub fn init_weight() -> WeightedAtomSweep<U64AtomHeader> {
    let settings = WeightedAtomSweepSettings {};

    WeightedAtomSweep::new(settings)
}
#[derive(Default)]
pub struct Traverse;

enum PathChoice {
    Value(()),
    Path(u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd)]
pub struct U64AtomHeader(pub i32);

impl From<i32> for U64AtomHeader {
    fn from(value: i32) -> Self {
        Self(value)
    }
}

// i32 ← U64AtomHeader
impl From<U64AtomHeader> for i32 {
    fn from(header: U64AtomHeader) -> Self {
        header.0 as i32
    }
}

impl AtomHeader for U64AtomHeader {}

/// Simple unit header for cases that don't need weighted atoms
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct UnitHeader;

impl AtomHeader for UnitHeader {}

impl From<i32> for UnitHeader {
    fn from(_: i32) -> Self {
        UnitHeader
    }
}

impl From<UnitHeader> for i32 {
    fn from(_: UnitHeader) -> Self {
        0
    }
}

pub fn next_atom<H>(mut z: ReadZipperTracked<H>) -> Result<AtomPosition, TraversalError>
where
    H: AtomHeader + From<i32> + Into<i32> + Copy + PartialOrd + Default,
{
    let child_agg_w: i32 = node_agg_w(z.fork_read_zipper()).unwrap();
    println!("starting next_atom with child_agg_w {child_agg_w}");

    // Handle case where there are no children weights
    if child_agg_w == 0 {
        return Ok(z.origin_path().to_vec());
    }

    let mut random_num = rand::random_range(0..child_agg_w);
    // println!("in next atom after checking child_agg_w with rand {random_num}");

    while z.child_count() >= 1 {
        if z.val().is_none() {
            z.descend_until();
        }

        // 5) returns a vec of tuple for agg of each child
        let choice_param = children_agg_w(z.fork_read_zipper()).unwrap();
        // println!("choice param {:?} on path {:?}", choice_param, String::from_utf8(z.origin_path().to_vec()).unwrap());

        // 6) choose based on the path and rand value
        let choice = choice(
            choice_param.to_vec(),
            &z.fork_read_zipper(),
            &mut random_num,
        );

        // 7) return if value is selected else conitnue to selected path
        let byte = match choice {
            PathChoice::Value(_) => {
                println!("chosen in next_atom {:?}", String::from_utf8_lossy(&z.origin_path().to_vec()));
                return Ok(z.origin_path().to_vec());
            }
            PathChoice::Path(b) => b,
        };

        // 8) descend to the path
        z.descend_to_byte(byte);
    }

    let path = z.origin_path();
    println!("chosen in next_atom {:?}", String::from_utf8_lossy(&path.to_vec()));
    Ok(path.to_vec())
}

fn children_agg_w<H>(mut path: ReadZipperUntracked<H>) -> Result<Vec<(i32, u8)>, Infallible>
where
    H: AtomHeader + From<i32> + Into<i32> + Copy + PartialOrd + Default,
{
    // 1) create a vector to store the agg_w of child and its mask
    let mut total: Vec<(i32, u8)> = Vec::new();

    for b in path.child_mask().iter() {
        path.descend_to_byte(b);
        let child = path.fork_read_zipper();
        let ch_agg_w = node_agg_w(child).unwrap();
        total.push((ch_agg_w, b));
        path.ascend_byte();
    }
    Ok(total)
}

fn node_agg_w<H>(path: ReadZipperUntracked<H>) -> Result<i32, TraversalError>
where
    H: AtomHeader + From<i32> + Into<i32> + Copy + PartialOrd + Default,
{
    let total: Result<i32, Infallible> = path.into_cata_jumping_side_effect_fallible(
        |_mask, children: &mut [i32], _size, maybe_v: Option<&H>, _path| {
            let from_children = children.iter().copied().sum::<i32>();
            let here: i32 = match maybe_v {
                Some(h) => (*h).into(),
                None => H::default().into(),
            };
            Ok(here + from_children)
        },
    );

    total.map_err(|e| match e {})
}

// return a Path(u8) if it choses path or Value(()) if it chosses value
fn choice<H>(
    mut choice_param: Vec<(i32, u8)>,
    path: &ReadZipperUntracked<H>,
    random_num: &mut i32,
) -> PathChoice
where
    H: AtomHeader + From<i32> + Into<i32> + Copy + PartialOrd + Default,
{
    // if path as a value chose between paths and values
    if let Some(value) = path.val() {
        if *random_num <= (*value).into() {
            return PathChoice::Value(());
        }
    }

    for (weight, path_byte) in choice_param.iter() {
        if *random_num < *weight {
            return PathChoice::Path(*path_byte);
        }
        *random_num -= *weight;
    }

    // println!("random_num {random_num} choice_param in choice {:?}", choice_param);
    PathChoice::Path(choice_param[0].1)
}
// Chunked priority queue

#[derive(Clone, Debug)]
struct AtomChunk {
    path: AtomPosition, // the path identifying this region (subtree root)
    score: i32,         // scoring metric (aggregate weight of subtree)
}

impl Eq for AtomChunk {}
impl PartialEq for AtomChunk {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score && self.path == other.path
    }
}

impl Ord for AtomChunk {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .cmp(&other.score)
            .then_with(|| other.path.cmp(&self.path))
    }
}
impl PartialOrd for AtomChunk {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug)]
pub struct ChunkedPQTraverse {
    heap: Arc<Mutex<BinaryHeap<AtomChunk>>>,
    depth: usize,
}

impl ChunkedPQTraverse {
    pub fn new(depth: usize) -> Self {
        Self {
            heap: Arc::new(Mutex::new(BinaryHeap::new())),
            depth,
        }
    }

    pub fn refresh(&self, z: &ReadZipperTracked<U64AtomHeader>) {
        let mut h = self.heap.lock().unwrap();
        h.clear();
        drop(h);

        let read_root = z.fork_read_zipper();
        self.collect_atoms_of_length_d(read_root, 0, self.depth, &self.heap);
    }

    fn node_agg_w_local(
        &self,
        path: ReadZipperUntracked<U64AtomHeader>,
    ) -> Result<i32, Infallible> {
        let total: Result<i32, Infallible> = path.into_cata_jumping_side_effect_fallible(
            |_mask, children: &mut [i32], _size, maybe_v: Option<&U64AtomHeader>, _path| {
                let from_children = children.iter().copied().sum::<i32>();
                let here = maybe_v.map(|h| h).unwrap_or(&U64AtomHeader::default()).0;
                Ok(here + from_children)
            },
        );

        total
    }
    // TODO: verify descend_first_k_path traverses alternative pathes
    //
    // fn collect_atoms_of_length_d(
    //     &self,
    //     mut z: ReadZipperUntracked<WeightedValue<U64AtomHeader>>,
    //     target_depth: usize,
    //     heap: &Arc<Mutex<BinaryHeap<AtomChunk>>>,
    // ) {
    //     if z.descend_first_k_path(target_depth) {
    //         loop {
    //             if let Some(_) = z.val() {
    //                 let score = self.node_agg_w_local(z.fork_read_zipper()).unwrap_or(0);
    //                 heap.lock().unwrap().push(AtomChunk {
    //                     path: z.origin_path().to_vec(),
    //                     score,
    //                 });
    //             }
    //
    //             if !z.to_next_k_path(target_depth) {
    //                 break;
    //             }
    //         }
    //     }
    // }
    //
    fn collect_atoms_of_length_d(
        &self,
        mut z: ReadZipperUntracked<U64AtomHeader>,
        cur: usize,
        target_depth: usize,
        heap: &Arc<Mutex<BinaryHeap<AtomChunk>>>,
    ) {
        if cur == target_depth {
            if z.val().is_some() {
                let score = self.node_agg_w_local(z.fork_read_zipper()).unwrap_or(0);
                heap.lock().unwrap().push(AtomChunk {
                    path: z.origin_path().to_vec(),
                    score,
                });
            }
            return;
        }

        // TODO: maybe remove if we dont add incomplete path
        //
        if z.child_count() == 0 {
            let score = self.node_agg_w_local(z.fork_read_zipper()).unwrap_or(0);
            heap.lock().unwrap().push(AtomChunk {
                path: z.origin_path().to_vec(),
                score,
            });
            return;
        }

        for b in z.child_mask().iter() {
            z.descend_to_byte(b);
            let child = z.fork_read_zipper();
            self.collect_atoms_of_length_d(child, cur + 1, target_depth, heap);
            z.ascend_byte();
        }
    }
}

impl Default for ChunkedPQTraverse {
    fn default() -> Self {
        ChunkedPQTraverse::new(3)
    }
}

impl ChunkedPQTraverse {
    pub fn next_atom(
        &self,
        z: ReadZipperTracked<U64AtomHeader>,
    ) -> Result<AtomPosition, Infallible> {
        {
            let mut h = self.heap.lock().unwrap();
            if h.is_empty() {
                drop(h);
                let read_root = z.fork_read_zipper();
                self.collect_atoms_of_length_d(read_root, 0, self.depth, &self.heap);
            }
        }

        let mut h = self.heap.lock().unwrap();
        match h.pop() {
            Some(chunk) => Ok(chunk.path),
            None => Ok(z.origin_path().to_vec()), // TODO: verify
        }
    }
}
