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
    
        let path1 = [1, 1, 1];
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
    
            // Allow ±15% tolerance
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

    /// Test traversal engine functionality
    #[test]
    pub fn test_traversal_engine_functionality() {
        println!("Testing TraversalEngine functionality...");

        // Create test map with standard U64AtomHeader
        let mut map = PathMap::<U64AtomHeader>::new();

        map.set_val_at(&[1, 2, 3], U64AtomHeader(5));
        map.set_val_at(&[4, 5, 6], U64AtomHeader(2));

        // Test next_atom method multiple times
        for _ in 0..10 {
            if let Ok(read_zipper) = map.zipper_head().read_zipper_at_borrowed_path(&[]) {
                // next_atom is a standalone function imported from weightedsweep
                match next_atom(read_zipper) {
                    Ok(atom_path) => {
                        println!(
                            "TraversalEngine found atom: {:?}",
                            atom_path
                        );
                    }
                    Err(e) => {
                        println!("TraversalEngine error: {:?}", e);
                    }  
                } 
            } else {
                println!("Failed to create read zipper");
            }
        }

        println!("TraversalEngine functionality test completed!");
    }

    /// Test concurrent access to WeightedMap
    #[test]
    pub fn test_concurrent_weighted_map_access() {
        println!("Testing concurrent WeightedMap access...");

        // Create PathMap with initial data
        let mut map = PathMap::<U64AtomHeader>::new();
        map.set_val_at(&[1, 1, 1], U64AtomHeader(50));

        // Arc for sharing zipper head across threads
        let head = Arc::new(map.into_zipper_head(&[]));

        // Spawn multiple threads accessing map
        let handles: Vec<_> = (0..5)
            .map(|i| {
                let head_clone = head.clone();
                thread::spawn(move || {
                    let path = vec![1 as u8, 1 as u8, 1 as u8];
                    match head_clone.read_zipper_at_borrowed_path(&path) {
                        Ok(zipper) => {
                            println!("Thread {}: Path access successful", i);
                            let header = zipper.val().expect("Weight value should exist");
                            assert_eq!(header.0, 50, "Thread {}: Unexpected weight value", i);
                        }
                        Err(e) => {
                            panic!("Thread {}: Path not found: {:?}", i, e);
                        }
                    }
                })
            })
            .collect();

        // Wait for all threads to complete and check if any panicked
        for handle in handles {
            assert!(handle.join().is_ok(), "A concurrent access thread panicked");
        }

        println!("Concurrent access test completed successfully!");
    }

    /// Test weight updates and propagation
    #[test]
    pub fn test_weight_propagation() {
        println!("Testing weight propagation...");

        // create initial map
        let mut map = PathMap::<U64AtomHeader>::new();

        let path1 = [1, 2, 3];
        let path2 = [1, 2, 4];
        let path3 = [1, 3, 1];

        map.set_val_at(&path1, U64AtomHeader(10));
        map.set_val_at(&path2, U64AtomHeader(20));
        map.set_val_at(&path3, U64AtomHeader(30));

        // check initial weight at leaf
        println!("Testing weight retrieval...");
        if let Ok(zipper) = map.zipper_head().read_zipper_at_borrowed_path(&path1) {
            if let Some(header) = zipper.val() {
                println!("Initial weight at {:?}: {}", path1, header.0);
                assert_eq!(header.0, 10);
            }
        }

        // update weight at leaf
        map.set_val_at(&path1, U64AtomHeader(50));

        // check updated weight
        if let Ok(zipper) = map.zipper_head().read_zipper_at_borrowed_path(&path1) {
            if let Some(header) = zipper.val() {
                println!("Updated weight at {:?}: {}", path1, header.0);
                assert_eq!(header.0, 50);
            }
        }

        println!("weight propagation test completed!");
    }

    // performance test with larger dataset
    #[test]
    pub fn test_performance_large_dataset() {
        println!("starting performance test...");

        let mut map = PathMap::<U64AtomHeader>::new();

        // Create dataset
        for i in 0..1000 {
            let path = vec![i as u8, (i / 10) as u8, (i / 100) as u8];
            map.set_val_at(&path, U64AtomHeader((i % 50) as i32 + 1));
        }

        let start_time = std::time::Instant::now();

        // Multiple traversals to test performance
        let mut successful_traversal = 0;
        for _ in 0..100 {
            if let Ok(read_zipper) = map.zipper_head().read_zipper_at_borrowed_path(&[]) {
                if next_atom(read_zipper).is_ok() {
                    successful_traversal += 1;
                }
            }
        }

        assert_eq!(successful_traversal, 100, "Should have performed 100 successful traversals");
        
        let duration = start_time.elapsed();
        println!("performance results: {} traversals in {:?}", successful_traversal, duration);
        println!("performance test complete")

    }

    #[test]
    pub fn test_error_handling() {
        println!("Testing error handling...");

        // Test with empty map
        let mut map = PathMap::<U64AtomHeader>::new();

        let head = map.zipper_head();
        if let Ok(read_zipper) = head.read_zipper_at_borrowed_path(&[]) {
            let result = next_atom(read_zipper);
            assert!(result.is_err() ||  result.as_ref().map_or(false, |p| p.is_empty()), "next_atom should return Err for an empty map, but returned {:?}", result);
        }

        // Test with non-existent paths
        let non_existent = [9, 9, 9];
        let zipper_result = head.read_zipper_at_borrowed_path(&non_existent);
        
        match zipper_result {
            Ok(z) => assert!(z.val().is_none(), "Path {:?} should not have a value", non_existent),
            Err(_) => (),
        }
        println!("Error handling test completed!");
    }

    /// -- this does not use the pathmap api yet!
    #[test]
    pub fn test_weighted_map_api() {

        let mut map = PathMap::<U64AtomHeader>::new();
        let path = [1u8, 2u8, 3u8];

        map.set_val_at(&path, U64AtomHeader(42));

        if let Ok(zipper) = map.zipper_head().read_zipper_at_borrowed_path(&path) {
            let val = zipper.val().expect("Value should exist at path");
            assert_eq!(val.0, 42);
        }

        map.set_val_at(&path, U64AtomHeader(100));
        if let Ok(zipper) = map.zipper_head().read_zipper_at_borrowed_path(&path) {
            let val = zipper.val().expect("Value should exist after update");
            assert_eq!(val.0, 100);
        }

        println!("Starting path iteration...");
        let mut count = 0;
        let head = map.zipper_head();
        if let Ok(mut read_zipper) = head.read_zipper_at_borrowed_path(&[]) {
            // to_next_val iterates through all values in the map
            while read_zipper.to_next_val() {
                count += 1;
                if let Some(val) = read_zipper.val() {
                    println!(
                        "  Found value at path {:?}: {:?}",
                        read_zipper.path(),
                        val.0
                    );
                }
            }
        }

        assert_eq!(count, 1, "Should have found exactly 1 value during iteration");
        println!("Found {} values during iteration", count);
        println!("Weighted Api test completed!");
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
