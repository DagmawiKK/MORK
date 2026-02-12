use core::borrow;
use pathmap::PathMap;
use pathmap::morphisms::Catamorphism;
use pathmap::zipper::{
    ReadZipperTracked, ReadZipperUntracked, Zipper, ZipperAbsolutePath, ZipperForking,
    ZipperIteration, ZipperMoving, ZipperValues,
};
use rand::Rng;
use std::cell::{Ref, RefCell};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::{
    convert::Infallible,
    error::Error,
    sync::{Arc, Mutex, OnceLock},
};
use weighted_atom_sweep::{
    AtomHeader, AtomPosition, KernelOperation, Operation, OperationObserver,
    SweepTransversalEngine, TransversalEngine, WeightedAtomSweep, WeightedAtomSweepSettings,
    WeightedMap, WeightedValue,
};

pub static GLOBAL_WS_SWEEP: OnceLock<Arc<WeightedAtomSweep<Traverse, Ops, U64AtomHeader>>> =
    OnceLock::new();

pub fn init_weight() -> WeightedAtomSweep<Traverse, Ops, U64AtomHeader> {
    let traversal = Arc::new(Traverse::default());
    let operations = vec![Ops::new("test".to_string())];
    let settings = WeightedAtomSweepSettings {};
    let map =
        WeightedMap::new(PathMap::<WeightedValue<U64AtomHeader>>::new().into_zipper_head(&[]));

    WeightedAtomSweep {
        traversal,
        operations,
        settings,
        map,
    }
}
#[derive(Default)]
pub struct Traverse;

enum PathChoice {
    Value(()),
    Path(u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd)]
pub struct U64AtomHeader(pub u64);

impl AtomHeader for U64AtomHeader {
    fn add(&self, other: &Self) -> Self {
        U64AtomHeader(&self.0 + other.0)
    }

    fn subtract(&self, other: &Self) -> Self {
        if self.0 > other.0 {
            return U64AtomHeader(&self.0 - other.0);
        }

        U64AtomHeader(0)
    }
}

impl TransversalEngine<WeightedValue<U64AtomHeader>> for Traverse {
    fn next_atom(
        &self,
        mut z: ReadZipperTracked<WeightedValue<U64AtomHeader>>,
    ) -> Result<AtomPosition, std::convert::Infallible> {
        let binding = WeightedValue::<U64AtomHeader>::default();
        let root_agg_w = z.val().unwrap_or(&binding);
        let U64AtomHeader(child_agg_w) = root_agg_w.child_agg_w.clone();
        // println!("in next atom {child_agg_w}");

        // Handle case where there are no children weights
        if child_agg_w == 0 {
            return Ok(z.origin_path().to_vec());
        }

        let mut random_num = rand::thread_rng().gen_range(0..child_agg_w);
        // println!("in next atom after checking child_agg_w with rand {random_num}");

        while z.child_count() >= 1 {
            if z.val().is_none() {
                z.descend_until();
            }

            // 5) returns a vec of tuple for agg of each child
            let choice_param: &Vec<(u64, u8)> = &self.children_agg_w(z.fork_read_zipper()).unwrap();
            // println!("choice param {:?} on path {:?}", choice_param, String::from_utf8(z.origin_path().to_vec()).unwrap());

            // 6) choose based on the path and rand value
            let choice = &self.choice(
                choice_param.to_vec(),
                &z.fork_read_zipper(),
                &mut random_num,
            );

            // 7) return if value is selected else conitnue to selected path
            let byte = match choice {
                PathChoice::Value(_) => {
                    // println!("chosen in next_atom {:?}", String::from_utf8(z.origin_path().to_vec()).unwrap());
                    return Ok(z.origin_path().to_vec());
                }
                PathChoice::Path(b) => b,
            };

            // 8) descend to the path
            z.descend_to_byte(*byte);
        }

        let path = z.origin_path();
        // println!("chosen in next_atom {:?}", String::from_utf8(path.to_vec()).unwrap());
        Ok(path.to_vec())
    }
}

impl SweepTransversalEngine<WeightedValue<U64AtomHeader>> for Traverse {}

impl Clone for Traverse {
    fn clone(&self) -> Self {
        Traverse
    }
}

impl std::fmt::Debug for Traverse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Traverse")
    }
}

impl Traverse {
    fn children_agg_w(
        &self,
        mut path: ReadZipperUntracked<WeightedValue<U64AtomHeader>>,
    ) -> Result<Vec<(u64, u8)>, Infallible> {
        // 1) create a vector to store the agg_w of child and its mask
        let mut total: Vec<(u64, u8)> = Vec::new();

        for b in path.child_mask().iter() {
            path.descend_to_byte(b);
            let child = path.fork_read_zipper();
            let ch_agg_w = &self.node_agg_w(child).unwrap();
            total.push((*ch_agg_w, b));
            path.ascend_byte();
        }
        Ok(total)
    }

    fn node_agg_w(
        &self,
        path: ReadZipperUntracked<WeightedValue<U64AtomHeader>>,
    ) -> Result<u64, Infallible> {
        let total: Result<u64, Infallible> = path.into_cata_jumping_side_effect_fallible(
            |_mask,
             children: &mut [u64],
             _size,
             maybe_v: Option<&WeightedValue<U64AtomHeader>>,
             _path| {
                let from_children = children.iter().copied().sum::<u64>();
                let here = maybe_v.map(|h| h.val).unwrap_or(U64AtomHeader::default()).0;
                Ok(here + from_children)
            },
        );

        total // .map(|s| s - root_val)
    }

    // return a Path(u8) if it choses path or Value(()) if it chosses value
    fn choice(
        &self,
        mut choice_param: Vec<(u64, u8)>,
        path: &ReadZipperUntracked<WeightedValue<U64AtomHeader>>,
        random_num: &mut u64,
    ) -> PathChoice {
        // if path as a value chose between paths and values
        if let Some(value) = path.val() {
            if *random_num <= value.val.0 {
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
}

// Chunked priority queue

#[derive(Clone, Debug)]
struct AtomChunk {
    path: AtomPosition, // the path identifying this region (subtree root)
    score: u64,         // scoring metric (aggregate weight of subtree)
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

    pub fn refresh(&self, z: &ReadZipperTracked<WeightedValue<U64AtomHeader>>) {
        let mut h = self.heap.lock().unwrap();
        h.clear();
        drop(h);

        let read_root = z.fork_read_zipper();
        self.collect_atoms_of_length_d(read_root, self.depth, &self.heap);
    }

    fn node_agg_w_local(
        &self,
        path: ReadZipperUntracked<WeightedValue<U64AtomHeader>>,
    ) -> Result<u64, Infallible> {
        let total: Result<u64, Infallible> = path.into_cata_jumping_side_effect_fallible(
            |_mask,
             children: &mut [u64],
             _size,
             maybe_v: Option<&WeightedValue<U64AtomHeader>>,
             _path| {
                let from_children = children.iter().copied().sum::<u64>();
                let here = maybe_v.map(|h| h.val).unwrap_or(U64AtomHeader::default()).0;
                Ok(here + from_children)
            },
        );

        total
    }

    fn collect_atoms_of_length_d(
        &self,
        mut z: ReadZipperUntracked<WeightedValue<U64AtomHeader>>,
        target_depth: usize,
        heap: &Arc<Mutex<BinaryHeap<AtomChunk>>>,
    ) {
        if z.descend_first_k_path(target_depth) {
            loop {
                if let Some(_) = z.val() {
                    let score = self.node_agg_w_local(z.fork_read_zipper()).unwrap_or(0);
                    heap.lock().unwrap().push(AtomChunk {
                        path: z.origin_path().to_vec(),
                        score,
                    });
                }

                if !z.to_next_k_path(target_depth) {
                    break;
                }
            }
        }
    }
}

impl Default for ChunkedPQTraverse {
    fn default() -> Self {
        ChunkedPQTraverse::new(3)
    }
}

impl SweepTransversalEngine<WeightedValue<U64AtomHeader>> for ChunkedPQTraverse {}

impl TransversalEngine<WeightedValue<U64AtomHeader>> for ChunkedPQTraverse {
    fn next_atom(
        &self,
        z: ReadZipperTracked<WeightedValue<U64AtomHeader>>,
    ) -> Result<AtomPosition, Infallible> {
        {
            let mut h = self.heap.lock().unwrap();
            if h.is_empty() {
                drop(h);
                let read_root = z.fork_read_zipper();
                self.collect_atoms_of_length_d(read_root, self.depth, &self.heap);
            }
        }

        let mut h = self.heap.lock().unwrap();
        match h.pop() {
            Some(chunk) => Ok(chunk.path),
            None => Ok(z.origin_path().to_vec()), // TODO: verify
        }
    }
}

pub struct Ops {
    name: String,
}

impl Ops {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}
impl Operation<WeightedValue<U64AtomHeader>> for Ops {
    fn name(&self) -> &str {
        &self.name
    }

    fn transform(&self, zipper: Arc<AtomPosition>) -> () {
        println!("found atom {:?}", zipper);
    }
}

impl KernelOperation<WeightedValue<U64AtomHeader>> for Ops {}

impl Clone for Ops {
    fn clone(&self) -> Self {
        Ops {
            name: self.name.clone(),
        }
    }
}

impl std::fmt::Debug for Ops {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ops({})", self.name)
    }
}

impl PartialEq for Ops {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}
