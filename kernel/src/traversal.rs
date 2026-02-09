use std::convert::Infallible;

use log::trace;
use pathmap::{morphisms::Catamorphism, zipper::{ReadZipperUntracked, Zipper, ZipperAbsolutePath, ZipperForking, ZipperMoving, ZipperValues}};

use crate::space::Space;

trait TraversalEngine<V>
    where V: Clone + Sync + Send {
    fn next_atom(&self, zipper: ReadZipperUntracked<V>) -> Option<Vec<u8>>; 
}

enum PathChoice {
    Value(()),
    Path(u8),
}

impl TraversalEngine<i32> for Space {

    fn next_atom(&self, mut z: ReadZipperUntracked<i32>) -> Option<Vec<u8>> {

        // 1) choose random number
        let mut root_agg_w = self.node_agg_w(z.fork_read_zipper()).unwrap();
        if  root_agg_w == 0 { return Some(z.origin_path().to_vec()); }
        let mut random_num = rand::random_range(0..=root_agg_w);

        // 2) start from zipper root
        while z.child_count() >= 1 {


            // 2) descend_until there is a value
            if z.val().is_none() {
                z.descend_until();
            }

            // 3) returns a vec of tuple for agg of each child
            let choice_param = &self.children_agg_w(z.fork_read_zipper()).unwrap();
            
            // 6) choose based on the path and rand value
            let choice = &self.choice(&mut random_num, choice_param.to_vec(), &z.fork_read_zipper());

            // 7) return if value is selected else conitnue to selected path
                let byte = match choice {
                PathChoice::Value(_) => {
                    trace!(target: "traverse", "{:?}", String::from_utf8(z.origin_path().to_vec()).unwrap());
                    return Some(z.origin_path().to_vec())
                },
                PathChoice::Path(b) => b
            };

            // 8) descend to the path
            z.descend_to_byte(*byte);
        }

        let path = z.origin_path();
        trace!(target: "traverse", "{:?}", String::from_utf8(path.to_vec()).unwrap());

        Some(path.to_vec())
    }
}
impl Space {
    fn children_agg_w(&self, mut path:ReadZipperUntracked<i32>)  -> Result<Vec<(i32, u8)>, Infallible> {
        // 1) create a vector to store the agg_w of child and its mask
        let mut total: Vec<(i32, u8)> = Vec::new();

        for b in path.child_mask().iter() {
            path.descend_to_byte(b);
            let child = path.fork_read_zipper();
            let ch_agg_w = &self.node_agg_w(child).unwrap();
            total.push((*ch_agg_w, b));
            path.ascend_byte();
        }
        Ok(total)
    }

    fn node_agg_w(&self, path: ReadZipperUntracked<i32>) -> Result<i32, Infallible>{

        let total: Result<i32, Infallible> = 
            path.into_cata_jumping_side_effect_fallible(|_mask, children: &mut [i32], _size, maybe_v: Option<&i32>, _path| {
                let from_children = children.iter().copied().sum::<i32>();
                let here = maybe_v.map(|h| h).unwrap_or(&0);
                Ok(here + from_children)
            });

        total// .map(|s| s - root_val)
    }


    // return a Path(u8) if it choses path or Value(()) if it chosses value
    fn choice(&self, random_num: &mut i32, mut choice_param: Vec<(i32, u8)>, path: &ReadZipperUntracked<i32>) -> PathChoice {
        // get agreagte weight of all values
        // let mut root_agg_w = self.node_agg_w(path.fork_read_zipper()).unwrap();
        
        // if path as a value chose between paths and values
        if path.val().is_some() {
            let focus_val = path.val().unwrap();
            // root_agg_w -= focus_val;
            if self.path_v_val(&focus_val, &choice_param, random_num) {
                return PathChoice::Value(())
            }
        }
        
        // Proper weighted random selection
        // let total_weight: i32 = choice_param.iter().map(|(w, _)| w).sum();
        // let mut random_num = rand::random_range(0..total_weight);
        let mut cumulative_weight = 0;
        
        // Sort by path for consistent iteration (not by weight)
        choice_param.sort_by_key(|(_, path)| *path);
        
        for (weight, path_byte) in choice_param.iter() {
            cumulative_weight += weight;
            if *random_num < cumulative_weight {
                return PathChoice::Path(*path_byte);
            }
        }
        
        // Fallback (shouldn't happen with proper algorithm)
        PathChoice::Path(choice_param[0].1)
            
    }

    // return true if its path false if value
    fn path_v_val(&self, focus_val: &i32, choice_param: &Vec<(i32, u8)>, random_num: &mut i32) -> bool {

        // let random_num = rand::random_range(0..=root_agg_w);
        let sum: i32 = choice_param.iter().map(|(i, _)| i).sum();

        if sum > *focus_val {
            if sum >= *random_num {
                true
            } else {
                false 
            }
        } else {
            if *focus_val >= *random_num {
                false
            } else {
                true
            }
        } 
    }

}
