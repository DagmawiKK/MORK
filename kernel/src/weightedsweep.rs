use pathmap::PathMap;
use pathmap::zipper::{ReadZipperTracked, ReadZipperUntracked, ZipperValues, ZipperForking, Zipper, ZipperMoving, ZipperAbsolutePath };
use pathmap::morphisms::Catamorphism;
use weighted_atom_sweep::{ AtomHeader, AtomPosition, KernelOperation, Operation, OperationObserver, SweepTransversalEngine, TransversalEngine, WeightedAtomSweep, WeightedAtomSweepSettings, WeightedMap };
use std::{convert::Infallible, error::Error, sync::{Arc, OnceLock, Mutex}};

pub static GLOBAL_WS_SWEEP: OnceLock<Arc<WeightedAtomSweep<Traverse, Ops, U64AtomHeader>>> = OnceLock::new();

pub fn init_weight() -> WeightedAtomSweep<Traverse, Ops, U64AtomHeader> {

    let traversal = Arc::new(Traverse::default());
    let operations = vec![Ops::new("test".to_string())];
    let settings = WeightedAtomSweepSettings {}; 
    let map = WeightedMap::new(PathMap::<U64AtomHeader>::new().into_zipper_head(&[]));

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

#[derive(Debug, Clone, Copy)]
pub struct U64AtomHeader(pub u64);

impl AtomHeader for U64AtomHeader {}

impl TransversalEngine<U64AtomHeader> for Traverse {
    fn next_atom(&self,mut z: ReadZipperTracked<U64AtomHeader>) 
        -> Result<AtomPosition, std::convert::Infallible> {
        
        while z.child_count() >= 1 {

            if z.val().is_none() {
                z.descend_until();
            }

            // 5) returns a vec of tuple for agg of each child
            let choice_param = &self.children_agg_w(z.fork_read_zipper()).unwrap();
            
            // 6) choose based on the path and rand value
            let choice = &self.choice(choice_param.to_vec(), &z.fork_read_zipper());

            // 7) return if value is selected else conitnue to selected path
                let byte = match choice {
                PathChoice::Value(_) => {
                    println!("{:?}", String::from_utf8(z.origin_path().to_vec()).unwrap());
                    return Ok(z.origin_path().to_vec())
                },
                PathChoice::Path(b) => b
            };

            // 8) descend to the path
            z.descend_to_byte(*byte);
        }

        let path = z.origin_path();
        println!("{:?}", String::from_utf8(path.to_vec()).unwrap());
        Ok(path.to_vec())
    }
}

impl SweepTransversalEngine<U64AtomHeader> for Traverse {}

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

    fn children_agg_w(&self, mut path:ReadZipperUntracked<U64AtomHeader>)  -> Result<Vec<(u64, u8)>, Infallible> {
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

    fn node_agg_w(&self, path: ReadZipperUntracked<U64AtomHeader>) -> Result<u64, Infallible>{

        let total: Result<u64, Infallible> = 
            path.into_cata_jumping_side_effect_fallible(|_mask, children: &mut [u64], _size, maybe_v: Option<&U64AtomHeader>, _path| {
                let from_children = children.iter().copied().sum::<u64>();
                let here = maybe_v.map(|h| h.0).unwrap_or(0);
                Ok(here + from_children)
            });

        total// .map(|s| s - root_val)
    }

    // return true if its path false if value
    fn path_v_val(focus_val: &u64, choice_param: &Vec<(u64, u8)>, root_agg_w :u64) -> bool {

        let random_num = rand::random_range(0..=root_agg_w);
        let sum: u64 = choice_param.iter().map(|(i, _)| i).sum();

        if sum > *focus_val {
            if sum >= random_num {
                true
            } else {
                false 
            }
        } else {
            if *focus_val >= random_num {
                false
            } else {
                true
            }
        } 
    }

    // return a Path(u8) if it choses path or Value(()) if it chosses value
    fn choice(&self, mut choice_param: Vec<(u64, u8)>, path: &ReadZipperUntracked<U64AtomHeader>) -> PathChoice {
        // get agreagte weight of all values
        let mut root_agg_w = self.node_agg_w(path.fork_read_zipper()).unwrap();
        
        // if path as a value chose between paths and values
        if path.val().is_some() {
            let focus_val = path.val().unwrap().0;
            root_agg_w -= focus_val;
            if !Traverse::path_v_val(&focus_val, &choice_param, root_agg_w) {
                return PathChoice::Value(())
            }
        }
        
        // Proper weighted random selection
        let total_weight: u64 = choice_param.iter().map(|(w, _)| w).sum();
        let mut random_num = rand::random_range(0..total_weight);
        let mut cumulative_weight = 0;
        
        // Sort by path for consistent iteration (not by weight)
        choice_param.sort_by_key(|(_, path)| *path);
        
        for (weight, path_byte) in choice_param.iter() {
            cumulative_weight += weight;
            if random_num < cumulative_weight {
                return PathChoice::Path(*path_byte);
            }
        }
        
        // Fallback (shouldn't happen with proper algorithm)
        PathChoice::Path(choice_param[0].1)
            
    }
        

}

pub struct Ops {
    name: String,
}

impl Ops {
    pub fn new(name: String) -> Self {
        Self {
            name
        }
    }
}
impl Operation<U64AtomHeader> for Ops {
    fn name(&self) -> &str {
        &self.name 
    } 

    fn transform(&self, zipper: Arc<AtomPosition>) -> () {
        println!("found atom {:?}", zipper);
    }
}

impl KernelOperation<U64AtomHeader> for Ops {}

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
