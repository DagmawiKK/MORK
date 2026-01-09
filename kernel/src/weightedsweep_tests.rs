use std::collections::HashSet;
use std::error::Error;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use crate::weightedsweep::{Ops, Traverse, U64AtomHeader};
use mork_interning::SharedMappingHandle;
use pathmap::morphisms::Catamorphism;
use pathmap::zipper::{ZipperCreation, ZipperIteration, ZipperMoving, ZipperValues, ZipperWriting};
use pathmap::PathMap;
use weighted_atom_sweep::{
    AtomHeader, AtomPosition, KernelOperation, Operation, OperationObserver,
    SweepTransversalEngine, TransversalEngine, WeightedAtomSweep, WeightedAtomSweepSettings,
    WeightedMap, WeightedValue,
};

// #[cfg(test)]
pub mod tests {
    use super::*;

    /// Enhanced LoggingOperation that properly handles Arc<Vec<u8>>
    #[derive(Debug, Clone)]
    pub struct LoggingOperation {
        name: String,
        transform_log: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl<H: AtomHeader> Operation<H> for LoggingOperation {
        fn name(&self) -> &str {
            &self.name
        }

        fn transform(&self, zipper: Arc<AtomPosition>) -> () {
            let path_str = String::from_utf8_lossy(&zipper);
            let log_entry = format!("Operation '{}' processed atom: {:?}", self.name, path_str);
            println!("🔧 {}", log_entry);

            if let Ok(mut logs) = self.transform_log.lock() {
                logs.push(log_entry);
            }
        }
    }

    impl KernelOperation<WeightedValue<U64AtomHeader>> for LoggingOperation {}

    impl PartialEq for LoggingOperation {
        fn eq(&self, other: &Self) -> bool {
            self.name == other.name
        }
    }

    /// Test basic WeightedAtomSweep integration with proper API usage
    pub fn test_weighted_atom_sweep_basic_integration() {
        println!("🚀 Starting basic WeightedAtomSweep integration test...");

        // Create test data with some weighted atoms
        let mut test_map = PathMap::<WeightedValue<U64AtomHeader>>::new();
        let mut test_map_z = test_map.into_zipper_head(&[]);
        let map = WeightedMap::<U64AtomHeader>::new(test_map_z);
        

        // Insert some test atoms with weights
        let path1 = vec![1, 2, 3];
        let path2 = vec![1, 4, 5];
        let path3 = vec![2, 1, 0];

        map.set_weighted_val(&path1, U64AtomHeader(50));
        map.set_weighted_val(&path2, U64AtomHeader(50));
        map.set_weighted_val(&path3, U64AtomHeader(50));

        // println!(
        //     "📊 Created test map with {} weighted atoms",
        //     map.inner.val_count()
        // );

        // Convert to weighted map with proper WeightedValue structure
        // let mut weighted_map = convert_to_weighted_map(test_map);
        // initialize_child_weights(&mut weighted_map);

        // Create traversal engine and operations
        let traverse = Traverse::default();
        let transform_log = Arc::new(std::sync::Mutex::new(Vec::new()));

        let operations = vec![
            LoggingOperation {
                name: "test_op_1".to_string(),
                transform_log: transform_log.clone(),
            },
            LoggingOperation {
                name: "test_op_2".to_string(),
                transform_log: transform_log.clone(),
            },
            LoggingOperation {
                name: "test_op_3".to_string(),
                transform_log: transform_log.clone(),
            },
            LoggingOperation {
                name: "test_op_4".to_string(),
                transform_log: transform_log.clone(),
            },
        ];

        let settings = WeightedAtomSweepSettings {};
        // let weighted_map_wrapper = WeightedMap::new(weighted_map.into_zipper_head(&[]));

        println!("🎯 Creating WeightedAtomSweep...");
        let sweep = WeightedAtomSweep::new(traverse, operations, settings, map);

        println!("🚂 Spawning background processes...");
        let zipper_head = sweep.spawn();

        println!("✅ Basic integration test completed successfully!");
        println!("📋 ZipperHead created and background threads spawned");

        // Give threads some time to work
        thread::sleep(Duration::from_millis(100));

        // Check transform log to see if operations were called
        match transform_log.lock() {
            Ok(logs) => {
                println!("📝 Operations were called {} times:", logs.len());
                for (i, log) in logs.iter().enumerate() {
                    println!("  {}. {}", i + 1, log);
                }
            }
            Err(e) => {
                println!("no lock found {e} ");
            }
        }

        // Verify zipper_head is accessible
        let read_zipper = zipper_head.read_zipper_at_borrowed_path(&[]);
        println!(
            "🔍 ZipperHead accessible - path exists: {}",
            read_zipper.is_ok()
        );
    }

    /// Test to verify traversal engine functionality with proper API usage
    pub fn test_traversal_engine_functionality() {
        println!("🚀 Testing TraversalEngine functionality...");

        // Create test map
        let mut map = PathMap::<WeightedValue<U64AtomHeader>>::new();
        let mut test_map_z = map.into_zipper_head(&[]);
        let mut test_map = WeightedMap::<U64AtomHeader>::new(test_map_z); 
        test_map.set_weighted_val(&[1, 2, 3], U64AtomHeader(5));
        test_map.set_weighted_val(&[4, 5, 6], U64AtomHeader(2));

        // Convert to weighted map
        // let mut weighted_map = convert_to_weighted_map(test_map);
        // initialize_child_weights(&mut weighted_map);
        // let weighted_map_wrapper = WeightedMap::new(weighted_map.into_zipper_head(&[]));

        let traverse = Traverse::default();

        // Test next_atom method multiple times
        for i in 0..10 {
            match test_map.inner.read_zipper_at_borrowed_path(&[]) {
                Ok(read_zipper) => match traverse.next_atom(read_zipper) {
                    Ok(atom_path) => {
                        println!(
                            "✅ TraversalEngine found atom: {:?}",
                            String::from_utf8_lossy(&atom_path)
                        );
                    }
                    Err(e) => {
                        println!("❌ TraversalEngine error: {:?}", e);
                    }
                },
                Err(e) => {
                    println!("❌ Failed to create read zipper: {:?}", e);
                }
            }
        }

        println!("✅ TraversalEngine functionality test completed!");
    }

    /// Test concurrent access to WeightedMap
    pub fn test_concurrent_weighted_map_access() {
        println!("🚀 Testing concurrent WeightedMap access...");

        // Create WeightedMap with initial data
        let mut map = PathMap::<WeightedValue<U64AtomHeader>>::new();
        let mut map_z = map.into_zipper_head(&[]);
        let mut test_map = WeightedMap::<U64AtomHeader>::new(map_z);

        test_map.set_weighted_val(&[1, 1, 1], U64AtomHeader(50));

        // let mut weighted_map = convert_to_weighted_map(test_map);
        // initialize_child_weights(&mut weighted_map);
        // let weighted_map_wrapper = WeightedMap::new(weighted_map.into_zipper_head(&[]));

        // Spawn multiple threads accessing map
        let handles: Vec<_> = (0..5)
            .map(|i| {
                let map_clone = test_map.inner.clone();
                thread::spawn(move || {
                    let path = vec![1 as u8, 1 as u8, 1 as u8];
                    match map_clone.read_zipper_at_borrowed_path(&path) {
                        Ok(zipper) => {
                            println!("Thread {}: Path access successful", i);
                            if let Some(header) = zipper.val() {
                                println!("Thread {}: Found weight value: {:?}", i, header.val);
                                println!(
                                    "Thread {}: Found child_agg_w: {:?}",
                                    i, header.child_agg_w
                                );
                            }
                        }
                        Err(_) => {
                            println!("Thread {}: Path not found", i);
                        }
                    }
                })
            })
            .collect();

        // Wait for all threads to complete
        for handle in handles {
            let _ = handle.join();
        }

        println!("✅ Concurrent access test completed!");
    }

    /// Test weight updates and propagation
    pub fn test_weight_propagation() {
        println!("🚀 Testing weight propagation...");

        // Create initial map
        let map = PathMap::<WeightedValue<U64AtomHeader>>::new();
        let map_z = map.into_zipper_head(&[]);
        let test_map = WeightedMap::<U64AtomHeader>::new(map_z);


        test_map.set_weighted_val(&[1, 2, 3], U64AtomHeader(10));
        test_map.set_weighted_val(&[1, 2, 4], U64AtomHeader(20));
        test_map.set_weighted_val(&[1, 3, 1], U64AtomHeader(30));

        // let mut weighted_map = convert_to_weighted_map(test_map);
        // initialize_child_weights(&mut weighted_map);
        // let weighted_map_wrapper = WeightedMap::new(weighted_map.into_zipper_head(&[]));

        // Test weight propagation
        println!("📊 Testing weight propagation...");

        // Check initial weights
        if let Some(weighted_val) = test_map.get_val(&[1, 2]) {
            println!(
                "Initial child_agg_w at [1,2]: {:?}",
                weighted_val.child_agg_w
            );
        }

        // Update weight at leaf
        test_map
            .set_weighted_val(&[1, 2, 3], U64AtomHeader(15))
            .unwrap();

        // Check updated weights
        if let Some(weighted_val) = test_map.get_val(&[1, 2]) {
            println!(
                "Updated child_agg_w at [1,2]: {:?}",
                weighted_val.child_agg_w
            );
        }

        println!("✅ Weight propagation test completed!");
    }

    /// Test weighted random distribution
    pub fn test_weighted_random_distribution() {
        println!("🚀 Testing weighted random distribution...");

        // Create map with different weights
        let map = PathMap::<WeightedValue<U64AtomHeader>>::new();
        let map_z = map.into_zipper_head(&[]);
        let mut test_map = WeightedMap::<U64AtomHeader>::new(map_z);

        test_map.set_weighted_val(&[1, 1, 1], U64AtomHeader(10)); // Low weight
        test_map.set_weighted_val(&[2, 2, 2], U64AtomHeader(20)); // High weight
        test_map.set_weighted_val(&[3, 3, 3], U64AtomHeader(50)); // Medium weight

        // let mut weighted_map = convert_to_weighted_map(test_map);
        // initialize_child_weights(&mut weighted_map);
        // let weighted_map_wrapper = WeightedMap::new(weighted_map.into_zipper_head(&[]));

        let traverse = Traverse::default();
        let mut atom_counts = std::collections::HashMap::new();

        // Run multiple traversals to test distribution
        for _ in 0..100 {
            if let Ok(read_zipper) = test_map.inner.read_zipper_at_borrowed_path(&[]) {
                if let Ok(atom_path) = traverse.next_atom(read_zipper) {
                    *atom_counts.entry(atom_path.clone()).or_insert(0) += 1;
                }
            }
        }

        println!("📊 Distribution results:");
        for (path, count) in &atom_counts {
            let percentage = (*count as f64 / 100.0) * 100.0;
            println!("  Path {:?}: {} times ({:.1}%)", path, count, percentage);
        }

        println!("✅ Weighted random distribution test completed!");
    }

    /// Full integration test combining all components
    pub fn test_full_weighted_atom_sweep_integration() {
        println!("🚀 Starting full WeightedAtomSweep integration test...");

        // Setup test data with hierarchical structure
        let mut map = PathMap::<WeightedValue<U64AtomHeader>>::new();
        let mut map_z = map.into_zipper_head(&[]);
        let test_map = WeightedMap::<U64AtomHeader>::new(map_z);

        // Create a small test data set with varied weights
        let atoms = vec![
            (vec![1, 1, 1], U64AtomHeader(10)),
            (vec![1, 1, 2], U64AtomHeader(12)),
            (vec![1, 1, 3], U64AtomHeader(17)),
            (vec![2, 1, 1], U64AtomHeader(5)),
            (vec![2, 2, 1], U64AtomHeader(15)),
        ];

        for (path, header) in &atoms {
            test_map.set_weighted_val(path, *header);
        }

        println!("📊 Created test map with {} atoms", atoms.len());

        // Create components
        let traverse = Traverse::default();
        let transform_log = Arc::new(std::sync::Mutex::new(Vec::new()));

        let operations = vec![LoggingOperation {
            name: "integration_test".to_string(),
            transform_log: transform_log.clone(),
        }];

        let settings = WeightedAtomSweepSettings {};
        let sweep = WeightedAtomSweep::new(traverse, operations, settings, test_map);

        println!("🚂 Spawning full integration test...");
        let zipper_head = sweep.spawn();

        // Allow background processing
        thread::sleep(Duration::from_millis(200));

        // Verify results
        if let Ok(logs) = transform_log.lock() {
            println!("📝 Background operations processed {} atoms", logs.len());
            for log in logs.iter().take(3) {
                // Show first 3 logs
                println!("  📋 {}", log);
            }
        }

        // Test map is still accessible
        let final_zipper = zipper_head.read_zipper_at_borrowed_path(&[]).unwrap();
        let total_atoms = final_zipper.val_count();
        println!("📊 Final map contains {} atoms", total_atoms);

        if total_atoms > 0 {
            println!("✅ Full integration test PASSED!");
        } else {
            println!("❌ Full integration test FAILED!");
        }
    }

    /// Performance test with larger dataset
    pub fn test_performance_with_large_dataset() {
        println!("🚀 Testing performance with large dataset...");

        let map = PathMap::<WeightedValue<U64AtomHeader>>::new();
        let map_z = map.into_zipper_head(&[]);
        let mut test_map = WeightedMap::<U64AtomHeader>::new(map_z);


        // Create larger dataset
        for i in 0..100 {
            let path = vec![(i % 10) as u8, ((i / 10) % 10) as u8, (i % 5) as u8];
            let weight = ((i * 7 + 13) % 100) as u64;
            test_map.set_weighted_val(&path, U64AtomHeader(weight));
        }

        let traverse = Traverse::default();
        let start_time = std::time::Instant::now();

        // Run multiple traversals
        let mut successful_traversals = 0;
        for _ in 0..1000 {
            if let Ok(read_zipper) = test_map.inner.read_zipper_at_borrowed_path(&[]) {
                if traverse.next_atom(read_zipper).is_ok() {
                    successful_traversals += 1;
                }
            }
        }

        let duration = start_time.elapsed();
        println!("📊 Performance results:");
        println!("  Total traversals: {}", successful_traversals);
        println!("  Total time: {:?}", duration);
        println!(
            "  Average time per traversal: {:?}",
            duration / successful_traversals
        );

        println!("✅ Performance test completed!");
    }

    /// Test error handling and edge cases
    pub fn test_error_handling() {
        println!("🚀 Testing error handling...");

        // Test with empty map
        let map = PathMap::<WeightedValue<U64AtomHeader>>::new();
        let map_z = map.into_zipper_head(&[]);
        let empty_map = WeightedMap::<U64AtomHeader>::new(map_z);

        let traverse = Traverse::default();

        if let Ok(read_zipper) = empty_map.inner.read_zipper_at_borrowed_path(&[]) {
            match traverse.next_atom(read_zipper) {
                Ok(_) => {
                    println!("⚠️ next_atom returns root in empty");
                }
                Err(e) => {
                    println!("✅ Expected error with empty map: {:?}", e);
                }
            }
        }

        // Test with non-existent paths
        if let Some(_) = empty_map.get_val(&[9, 9, 9]) {
            println!("⚠️  Unexpected value at non-existent path");
        } else {
            println!("✅ Correctly returned None for non-existent path");
        }

        println!("✅ Error handling test completed!");
    }

    /// Test WeightedMap API methods directly
    pub fn test_weighted_map_api() {
        println!("🚀 Testing WeightedMap API methods...");

        let map = PathMap::<WeightedValue<U64AtomHeader>>::new();
        let map_z = map.into_zipper_head(&[]);
        let mut test_map = WeightedMap::<U64AtomHeader>::new(map_z);

        test_map.set_weighted_val(&[1, 2, 3], U64AtomHeader(42));

        // let mut weighted_map = convert_to_weighted_map(test_map);
        // initialize_child_weights(&mut weighted_map);
        // let weighted_map_wrapper = WeightedMap::new(weighted_map.into_zipper_head(&[]));

        // Test get_val
        if let Some(weighted_val) = test_map.get_val(&[1, 2, 3]) {
            println!(
                "✅ get_val successful: val={:?}, child_agg_w={:?}",
                weighted_val.val, weighted_val.child_agg_w
            );
        }

        // Test set_weighted_val
        test_map
            .set_weighted_val(&[1, 2, 3], U64AtomHeader(100))
            .unwrap();
        if let Some(weighted_val) = test_map.get_val(&[1, 2, 3]) {
            println!("✅ set_weighted_val successful: val={:?}", weighted_val.val);
        }

        // Test path iteration
        let mut read_zipper = test_map.read_zipper_at_path(&[]).unwrap();
        let mut count = 0;
        while read_zipper.to_next_val() {
            count += 1;
            if let Some(val) = read_zipper.val() {
                println!(
                    "  Found value at path {:?}: {:?}",
                    String::from_utf8_lossy(read_zipper.path()),
                    val
                );
            }
        }

        println!("✅ Found {} values during iteration", count);
        println!("✅ WeightedMap API test completed!");
    }
}
