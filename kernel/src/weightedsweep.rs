use pathmap::morphisms::Catamorphism;
use pathmap::zipper::{
    ReadZipperTracked, ReadZipperUntracked, Zipper, ZipperAbsolutePath, ZipperForking,
    ZipperMoving, ZipperValues,
};
use pathmap::PathMap;
use std::{
    convert::Infallible,
    error::Error,
    sync::{Arc, Mutex, OnceLock},
};
use weighted_atom_sweep::{
    WeightedAtomSweep, AtomHeader, WeightedAtomSweepSettings, AtomPosition, TraversalEngine, TraversalError
};

pub static GLOBAL_WS_SWEEP: OnceLock<Arc<WeightedAtomSweep<U64AtomHeader>>> =
    OnceLock::new();

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

pub fn next_atom<H>(
    mut z: ReadZipperTracked<H>,
) -> Result<AtomPosition, TraversalError> 
where
    H: AtomHeader + From<i32> + Into<i32> + Copy + PartialOrd + Default
{
    let binding  = H::default();
    let root_agg_w = z.val().unwrap_or(&binding);
    let child_agg_w:i32 = root_agg_w.clone().into();
    // println!("in next atom {child_agg_w}");

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
        let choice = choice(choice_param.to_vec(), &z.fork_read_zipper(), &mut random_num);

        // 7) return if value is selected else conitnue to selected path
        let byte = match choice {
            PathChoice::Value(_) => {
                // println!("chosen in next_atom {:?}", String::from_utf8(z.origin_path().to_vec()).unwrap());
                return Ok(z.origin_path().to_vec())
            },
            PathChoice::Path(b) => b
        };

        // 8) descend to the path
        z.descend_to_byte(byte);
    }

    let path = z.origin_path();
    // println!("chosen in next_atom {:?}", String::from_utf8(path.to_vec()).unwrap());
    Ok(path.to_vec())
}

fn children_agg_w<H>(mut path: ReadZipperUntracked<H>) -> Result<Vec<(i32, u8)>, Infallible>
where
    H: AtomHeader + From<i32> + Into<i32> + Copy + PartialOrd + Default
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

fn node_agg_w<H>(
    path: ReadZipperUntracked<H>,
) -> Result<i32, TraversalError> 
where 
    H: AtomHeader + From<i32> + Into<i32> + Copy + PartialOrd + Default
{
    let total: Result<i32, Infallible> = path.into_cata_jumping_side_effect_fallible(
        |_mask,
         children: &mut [i32],
         _size,
         maybe_v: Option<&H>,
         _path| {
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
    H: AtomHeader + From<i32> + Into<i32> + Copy + PartialOrd + Default
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

