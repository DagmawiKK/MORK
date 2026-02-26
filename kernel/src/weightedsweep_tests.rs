use std::collections::HashSet;
use std::error::Error;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use crate::weightedsweep::{ChunkedPQTraverse, Traverse, U64AtomHeader, next_atom};
use mork_interning::SharedMappingHandle;
use pathmap::PathMap;
use pathmap::morphisms::Catamorphism;
use pathmap::zipper::{ZipperCreation, ZipperIteration, ZipperMoving, ZipperValues, ZipperWriting, WriteZipperTracked};
use weighted_atom_sweep::{
    AtomHeader, AtomPosition, Operation, OperationObserver, WeightedAtomSweep,
    WeightedAtomSweepSettings, TraversalEngine
};

mod operations {
    use super::*;

    pub fn log_atom(_wz: &mut WriteZipperTracked<U64AtomHeader>, _atom_path: &[u8]) {
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }
}

#[cfg(test)]
mod random_walk {
    use super::*;

    /// Test weighted random distribution with empty pathmap 
    #[test]
    fn test_weighted_empty_pathmap() {
    
        // Create Map
        let mut map = PathMap::<U64AtomHeader>::new();

        let atom = match map.zipper_head().read_zipper_at_borrowed_path(&[]) {
            Ok(read_zipper) => next_atom(read_zipper),
            Err(e) => panic!("{:?}", e)

        };

        assert_eq!(atom.is_ok(), true);
    }

    /// Test weighted random distribution with each path being unique
    #[test]
    fn test_weighted_random_distribution_unique_path() {

        // Create map with different weights
        let mut map = PathMap::<U64AtomHeader>::new();

        let path1 = [1, 1, 1];
        let path2 = [2, 2, 2];
        let path3 = [3, 3, 3];
        map.set_val_at(&path1, U64AtomHeader(10)); // Low weight
        map.set_val_at(&path2, U64AtomHeader(20)); // High weight
        map.set_val_at(&path3, U64AtomHeader(50)); // Medium weight

        let mut atom_counts = std::collections::HashMap::new();

        // Run multiple traversals to test distribution
        for _ in 0..100 {
            if let Ok(read_zipper) = map.zipper_head().read_zipper_at_borrowed_path(&[]) {
                if let Ok(atom_path) = next_atom(read_zipper) {
                    println!("found atom_path {:?}", atom_path);
                    *atom_counts.entry(atom_path.clone()).or_insert(0) += 1;
                }
            }
        }

        for (path, count) in &atom_counts {
            let percentage = (*count as f64 / 100.0) * 100.0;
             // Get expected percentage based on path
            let expected = match path {
                p if p == &[1, 1, 1] => 12.5,
                p if p == &[2, 2, 2] => 25.0,
                p if p == &[3, 3, 3] => 62.5,
                _ => 0.0,
            };
    
            // Allow ±15% tolerance (adjust as needed)
            assert!((percentage - expected).abs() < 15.0, 
                "Path {:?} has {}%, expected {}%", path, percentage, expected);
            }
    }
    
    /// Test weighted random distribution with overlapping paths
    #[test]
    fn test_weighted_random_distribution_overlapping_path() {
    
        // Create map with different weights
        let mut map = PathMap::<U64AtomHeader>::new();
    
        let path1 = [1, 1, 2];
        let path2 = [1, 1, 2];
        let path3 = [2, 1, 1];
        let path4 = [2, 1, 2];
        let path5 = [2, 2, 2];
        let path6 = [2, 2, 1];
    
        let atoms = vec![
            (&path1, U64AtomHeader(10)),
            (&path2, U64AtomHeader(30)),
            (&path3, U64AtomHeader(15)),
            (&path4, U64AtomHeader(2)),
            (&path5, U64AtomHeader(8)),
            (&path6, U64AtomHeader(20)),
        ];


        let total_weight: u64 = atoms.iter().map(|(_, h) | h.0 as u64).sum(); // changd h.0 to h.0 as u64 to avoid casting error(i32)
    
        for (path, header) in &atoms {
            map.set_val_at(path, *header);
        }
    
        let mut atom_counts = std::collections::HashMap::new();
    
        // Run multiple traversals to test distribution
        for _ in 0..100 {
            if let Ok(read_zipper) = map.zipper_head().read_zipper_at_borrowed_path(&[]) {
                if let Ok(atom_path) = next_atom(read_zipper) {
                    println!("found atom_path {:?}", atom_path);
                    *atom_counts.entry(atom_path.clone()).or_insert(0) += 1;
                }
            }
        }
    
        for (path, count) in &atom_counts {
            let percentage = (*count as f64 / 100.0) * 100.0;
            println!("for path {:?} found {count} making {percentage}", path);

            // calculate percentage based on weight
            let weight = atoms.iter().find(|(p, _)| p.as_slice() == path.as_slice()).map(|(_, h)| h.0).unwrap_or(0);

            let expected = (weight as f64 / total_weight as f64) * 100.0;
            println!("path: {:?}, count: {}, percentage {}, expected: {}%", path, count, percentage, expected);
    
            // Allow ±15% tolerance (adjust as needed)
            assert!((percentage - expected).abs() < 15.0, 
                "Path {:?} has {}%, expected {}%", path, percentage, expected);
        }
    }


    /// Test basic WeightedAtomSweep integration with proper API usage
    #[test]
    fn test_weighted_atom_sweep_basic_integration() {

        // Create test data with some weighted atoms
        let mut map = PathMap::<U64AtomHeader>::new();

        // Insert some test atoms with weights
        let path1 = vec![1, 2, 3];
        let path2 = vec![1, 4, 5];
        let path3 = vec![2, 1, 0];

        map.set_val_at(&path1, U64AtomHeader(50));
        map.set_val_at(&path2, U64AtomHeader(50));
        map.set_val_at(&path3, U64AtomHeader(50));

        assert_eq!(map.val_count(), 3);

        // Create traversal engine and operations
        let settings = WeightedAtomSweepSettings {};
        // let weighted_map_wrapper = WeightedMap::new(weighted_map.into_zipper_head(&[]));

        // Creating WeightedAtomSweep
        let mut sweep = WeightedAtomSweep::<U64AtomHeader>::new(settings);

        let engine1 = TraversalEngine::new("engine1", next_atom);
        let process1 = sweep.add_engine(engine1);

        let log_op = Operation::<U64AtomHeader>::new("log_atom", operations::log_atom);

        process1.subscribe(log_op);

        // Create and add second engine with operations
        let controller = sweep.spawn();

        // Let it run briefly then shutdown
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let result = controller.shutdown();

        assert!(result.is_ok(), "sweep shutdown should succeed");

    }
}

#[cfg(test)]
mod chunked_pq_test {
    use super::*;
    use pathmap::zipper::ZipperForking;
    use pathmap::zipper_tracking::PathStatus;
    use pathmap::{PathMap, zipper};
    use std::iter::zip;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;
    use weighted_atom_sweep::WeightedAtomSweepSettings;

    #[test]
    fn test_basic_collection() {
        let mut map = PathMap::<U64AtomHeader>::new();

        map.set_val_at(&[1, 1], U64AtomHeader(10));
        map.set_val_at(&[1, 2], U64AtomHeader(20));
        map.set_val_at(&[2, 1], U64AtomHeader(30));
        map.set_val_at(&[2, 2], U64AtomHeader(40));

        let traverse = ChunkedPQTraverse::new(2);

        let mut map_z = map.into_zipper_head(&[]);
        let zipper = map_z.read_zipper_at_borrowed_path(&[]).unwrap();

        let atom1 = traverse.next_atom(zipper.clone()).unwrap();
        let atom2 = traverse.next_atom(zipper.clone()).unwrap();
        let atom3 = traverse.next_atom(zipper.clone()).unwrap();
        let atom4 = traverse.next_atom(zipper.clone()).unwrap();
        assert_eq!(atom1.len(), 2, "Atom 1 should have length 2");
        assert_eq!(atom2.len(), 2, "Atom 2 should have length 2");
        assert_eq!(atom3.len(), 2, "Atom 3 should have length 2");
        assert_eq!(atom4.len(), 2, "Atom 4 should have length 2");
    }

    #[test]
    fn test_priority_ordering() {
        let mut map = PathMap::<U64AtomHeader>::new();

        map.set_val_at(&[1, 1], U64AtomHeader(10));
        map.set_val_at(&[2, 1], U64AtomHeader(100));
        map.set_val_at(&[3, 1], U64AtomHeader(50));

        let traverse = ChunkedPQTraverse::new(2);

        let mut map_z = map.into_zipper_head(&[]);
        let zipper = map_z.read_zipper_at_borrowed_path(&[]).unwrap();

        let first = traverse.next_atom(zipper.clone()).unwrap();
        assert_eq!(
            first,
            vec![2, 1],
            "Heighest weight atoom should be returned first"
        );
    }
    //
    #[test]
    fn test_empty_trie() {
        let mut map = PathMap::<U64AtomHeader>::new();

        let traverse = ChunkedPQTraverse::new(2);
        let mut map_z = map.into_zipper_head(&[]);
        let zipper = map_z.read_zipper_at_borrowed_path(&[]).unwrap();

        let atom = traverse.next_atom(zipper).unwrap();

        assert_eq!(
            atom,
            Vec::<u8>::new(),
            "Empty trie should return empty path"
        );
    }

    #[test]
    fn test_depth_boundaries() {
        let mut map = PathMap::<U64AtomHeader>::new();

        map.set_val_at(&[1, 1, 1, 1], U64AtomHeader(10));
        map.set_val_at(&[2, 2, 2, 2], U64AtomHeader(20));

        let mut map_z = map.into_zipper_head(&[]);
        let zipper = map_z.read_zipper_at_borrowed_path(&[]).unwrap();

        let traverse_d2 = ChunkedPQTraverse::new(2);
        let atom_d2 = traverse_d2.next_atom(zipper).unwrap();
        assert_eq!(
            atom_d2,
            Vec::<u8>::new(),
            "Depth 2 should return empty for atoms at depth 4"
        );

        let traverse_d4 = ChunkedPQTraverse::new(4);
        let zipper_d4 = map_z.read_zipper_at_borrowed_path(&[]).unwrap();
        let atom_d4 = traverse_d4.next_atom(zipper_d4).unwrap();
        assert_eq!(atom_d4.len(), 4, "Depth 4 should return atoms of length 4");
    }

    #[test]
    fn test_refresh() {
        let mut map = PathMap::<U64AtomHeader>::new();

        map.set_val_at(&[1, 1], U64AtomHeader(10));
        map.set_val_at(&[2, 1], U64AtomHeader(10));
        map.set_val_at(&[3, 1], U64AtomHeader(30));
        map.set_val_at(&[4, 1], U64AtomHeader(40));

        let traverse = ChunkedPQTraverse::new(2);
        let mut map_z = map.into_zipper_head(&[]);
        let zipper = map_z.read_zipper_at_borrowed_path(&[]).unwrap();

        let atom1 = traverse.next_atom(zipper.clone()).unwrap();
        let atom2 = traverse.next_atom(zipper.clone()).unwrap();

        traverse.refresh(&zipper);

        let atom_new = traverse.next_atom(zipper.clone()).unwrap();
        assert!(atom_new == vec![3, 1] || atom_new == vec![4, 1]);
    }

    #[test]
    fn test_single_atom() {
        let mut map = PathMap::<U64AtomHeader>::new();

        map.set_val_at(&[1, 1], U64AtomHeader(10));

        let traverse = ChunkedPQTraverse::new(2);

        let mut map_z = map.into_zipper_head(&[]);
        let zipper = map_z.read_zipper_at_borrowed_path(&[]).unwrap();

        let atom1 = traverse.next_atom(zipper.clone()).unwrap();
        let atom2 = traverse.next_atom(zipper.clone()).unwrap();

        assert_eq!(atom1, vec![1, 1]);
        assert_eq!(atom2, vec![1, 1]); // TODO: maybe don't recollect on empty heap
    }
    //
    #[test]
    fn test_same_weight() {
        let mut map = PathMap::<U64AtomHeader>::new();

        map.set_val_at(&[1, 1], U64AtomHeader(50));
        map.set_val_at(&[1, 1], U64AtomHeader(50));
        map.set_val_at(&[1, 1], U64AtomHeader(50));

        let traverse = ChunkedPQTraverse::new(2);
        let mut map_z = map.into_zipper_head(&[]);
        let zipper = map_z.read_zipper_at_borrowed_path(&[]).unwrap();

        let atoms: Vec<_> = (0..3)
            .filter_map(|_| traverse.next_atom(zipper.clone()).ok())
            .take(5)
            .collect();

        assert_eq!(atoms.len(), 3);
    }

    #[test]
    fn test_wide_trie() {
        let mut map = PathMap::<U64AtomHeader>::new();

        for i in 1..=10 {
            map.set_val_at(&[i, 1], U64AtomHeader(i.into()));
        }

        let mut map_z = map.into_zipper_head(&[]);
        let traverse = ChunkedPQTraverse::new(2);

        let zipper = map_z.read_zipper_at_borrowed_path(&[]).unwrap();

        let atoms: Vec<_> = (0..10)
            .filter_map(|_| traverse.next_atom(zipper.clone()).ok())
            .take(15)
            .collect();

        assert_eq!(atoms.len(), 10);
    }

    #[test]
    fn test_descending_order() {
        let mut map = PathMap::<U64AtomHeader>::new();

        map.set_val_at(&[1, 1], U64AtomHeader(100));
        map.set_val_at(&[2, 1], U64AtomHeader(80));
        map.set_val_at(&[3, 1], U64AtomHeader(60));
        map.set_val_at(&[4, 1], U64AtomHeader(40));
        map.set_val_at(&[5, 1], U64AtomHeader(20));

        let traverse = ChunkedPQTraverse::new(2);
        let map_z = map.into_zipper_head(&[]);
        let zipper = map_z.read_zipper_at_borrowed_path(&[]).unwrap();

        let first = traverse.next_atom(zipper.clone()).unwrap();
        assert_eq!(first, vec![1, 1]);

        let second = traverse.next_atom(zipper.clone()).unwrap();
        assert_eq!(second, vec![2, 1]);
    }
}
