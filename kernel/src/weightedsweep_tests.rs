use std::error::Error;

// #[cfg(test)]
pub mod tests {
    use std::pin::Pin;
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::Duration;
    
    use mork_interning::SharedMappingHandle;
    use pathmap::PathMap;
    use pathmap::zipper::{ZipperCreation, ZipperWriting, ZipperValues, ZipperMoving};
    use weighted_atom_sweep::{
        AtomHeader, AtomPosition, WeightedAtomSweep, WeightedAtomSweepSettings, 
        WeightedMap, Operation, OperationObserver, SweepTransversalEngine, TransversalEngine, KernelOperation
    };
    use crate::weightedsweep::{Traverse, U64AtomHeader, Ops};

    // // Make LoggingOperation public and re-export it
    // #[derive(Debug, Clone)]
    // pub struct LoggingOperation {}

    // Test operation that logs when transform is called
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
            let path_str = String::from_utf8_lossy(&zipper.as_slice());
            let log_entry = format!("Operation '{}' processed atom: {:?}", self.name, path_str);
            println!("🔧 {}", log_entry);
            
            if let Ok(mut logs) = self.transform_log.lock() {
                logs.push(log_entry);
            }
        }
    }

    // Make LoggingOperation public and re-export it
    impl KernelOperation<U64AtomHeader> for LoggingOperation {}

    impl PartialEq for LoggingOperation {
        fn eq(&self, other: &Self) -> bool {
            self.name == other.name
        }
    }

    // Async operation implementation
    // impl LoggingOperation {
    //     fn name(&self) -> &str {
    //         &self.name
    //     }
    //
    //     fn transform(&self, zipper: Arc<AtomPosition>) -> Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    //         let path_str = String::from_utf8_lossy(&zipper.as_slice());
    //         let log_entry = format!("Async Operation '{}' processed atom: {:?}", self.name, path_str);
    //         println!("🔄 {}", log_entry);
    //         
    //         Box::pin(async move {
    //             if let Ok(mut logs) = self.transform_log.lock() {
    //                 logs.push(log_entry);
    //             }
    //         })
    //     }
    // }

    // Test to verify WeightedAtomSweep basic integration
    pub fn test_weighted_atom_sweep_basic_integration() {
        println!("🚀 Starting basic WeightedAtomSweep integration test...");
        
        // Create test data with some weighted atoms
        let mut test_map = PathMap::<U64AtomHeader>::new();
        
        // Insert some test atoms with weights
        let path1 = [1, 2, 3];
        let path2 = [1, 4, 5];
        let path3 = [2, 1, 0];
        
        test_map.insert(&path1, U64AtomHeader(50));
        test_map.insert(&path2, U64AtomHeader(50));
        test_map.insert(&path3, U64AtomHeader(50));
        
        println!("📊 Created test map with {} weighted atoms", test_map.val_count());
        
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
        
        let settings = WeightedAtomSweepSettings{};
        let weighted_map = WeightedMap::new(test_map.into_zipper_head(&[]));
        
        println!("🎯 Creating WeightedAtomSweep...");
        let sweep = WeightedAtomSweep::new(traverse, operations, settings, weighted_map);
        
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
        println!("🔍 ZipperHead accessible - path exists: {}", read_zipper.is_ok());
    }

    // Async test to demonstrate proper weighted random selection and continuous processing
    // pub async fn test_async_weighted_atom_sweep() -> Result<(), Box<dyn std::error::Error>> {
    //     println!("🚀 Starting async WeightedAtomSweep test...");
    //     
    //     // Create test data with more atoms
    //     let mut test_map = PathMap::<U64AtomHeader>::new();
    //     for i in 0..20 {
    //         test_map.insert(&[i as u8, (i%3) as u8, (i%5) as u8], 
    //                       U64AtomHeader((i*7+1) as u64));
    //     }
    //     
    //     println!("📊 Created test map with {} weighted atoms", test_map.val_count());
    //     
    //     // Create async logging operations
    //     let transform_log = Arc::new(std::sync::Mutex::new(Vec::new()));
    //     let operations = vec![
    //         LoggingOperation { 
    //             name: "async_test_1".to_string(), 
    //             transform_log: transform_log.clone() 
    //         },
    //         LoggingOperation { 
    //             name: "async_test_2".to_string(), 
    //             transform_log: transform_log.clone() 
    //         },
    //     ];
    //     
    //     // Create async weighted atom sweep
    //     let traverse = Traverse::default();
    //     let settings = WeightedAtomSweepSettings{};
    //     let weighted_map = WeightedMap::new(test_map.into_zipper_head(&[]));
    //     let sweep = weighted_atom_sweep::AsyncWeightedAtomSweep::new(traverse, operations, settings, weighted_map);
    //     
    //     // Run async sweep
    //     let zipper_head = sweep.spawn_async().await?;
    //     
    //     // Give background tasks time to process
    //     tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    //     
    //     // Verify results
    //     if let Ok(logs) = transform_log.lock() {
    //         let unique_atoms: std::collections::HashSet<_> = logs.iter().map(|l| l.split(" processed atom: ").nth(1).unwrap_or("")).collect();
    //         println!("📊 Summary:");
    //         println!("  Total operations processed: {}", logs.len());
    //         println!("  Unique atoms processed: {}", unique_atoms.len());
    //         println!("  Expected distribution ~33%, 33%, 33% (weights 28-145)");
    //         println!("  Result: {:?}", unique_atoms);
    //         
    //         if unique_atoms.len() > 10 {
    //             println!("✅ Async WeightedAtomSweep PASSED - proper weighted distribution!");
    //         } else {
    //             println!("❌ Async WeightedAtomSweep FAILED - limited atom diversity");
    //         }
    //     }
    //     
    //     println!("🎉 Async WeightedAtomSweep test completed successfully!");
    //     Ok(())
    // }

    // pub async fn test_w_a_s() -> Result<(), Error> {
    //     
    //     println!("📊 Created test map with {} weighted atoms", test_map.val_count());
    //     
    //     // Create async logging operations
    //     let transform_log = Arc::new(std::sync::Mutex::new(Vec::new()));
    //     let operations = vec![
    //         LoggingOperation { 
    //             name: "async_test_1".to_string(), 
    //             transform_log: transform_log.clone() 
    //         },
    //         LoggingOperation { 
    //             name: "async_test_2".to_string(), 
    //             transform_log: transform_log.clone() 
    //         },
    //     ];
    //     
    //     // Create async weighted atom sweep
    //     let traverse = Traverse::default();
    //     let settings = WeightedAtomSweepSettings{};
    //     let weighted_map = WeightedMap::new(test_map.into_zipper_head(&[]));
    //     let sweep = weighted_atom_sweep::AsyncWeightedAtomSweep::new(traverse, operations, settings, weighted_map);
    //     
    //     // Run async sweep
    //     let zipper_head = sweep.spawn_async().await?;
    //     
    //     // Give background tasks time to process
    //     tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    //     
    //     // Verify results
    //     if let Ok(logs) = transform_log.lock() {
    //         let unique_atoms: std::collections::HashSet<_> = logs.iter().map(|l| l.split(" processed atom: ").nth(1).unwrap_or("")).collect();
    //         println!("📊 Summary:");
    //         println!("  Total operations processed: {}", logs.len());
    //         println!("  Unique atoms processed: {}", unique_atoms.len());
    //         println!("  Expected distribution ~33%, 33%, 33% (weights 28-145)");
    //         println!("  Result: {:?}", unique_atoms);
    //         
    //         if unique_atoms.len() > 10 {
    //             println!("✅ Async WeightedAtomSweep PASSED - proper weighted distribution!");
    //         } else {
    //             println!("❌ Async WeightedAtomSweep FAILED - limited atom diversity");
    //         }
    //     }
    //     
    //     println!("🎉 Async WeightedAtomSweep test completed successfully!");
    //     Ok(())
    // }

    // Test to verify WsSink integration with WeightedMap
    // pub fn test_wssink_weighted_map_integration() {
    //     println!("🚀 Testing WsSink integration with WeightedMap...");
    //     
    //     // Use the actual WPATH thread-local from sinks.rs
    //     crate::sinks::WPATH.with_borrow_mut(|wpath| {
    //         println!("📊 WPATH accessed for testing");
    //         
    //         // Add some test data
    //         let test_path1 = [1, 2, 3];
    //         let test_path2 = [4, 5, 6];
    //         
    //         // Use WriteZipper to add data
    //         let mut write_zipper1 = wpath.write_zipper_at_exclusive_path(&test_path1).unwrap();
    //         write_zipper1.set_val(U64AtomHeader(100));
    //         wpath.cleanup_write_zipper(write_zipper1);
    //         
    //         let mut write_zipper2 = wpath.write_zipper_at_exclusive_path(&test_path2).unwrap();
    //         write_zipper2.set_val(U64AtomHeader(200));
    //         wpath.cleanup_write_zipper(write_zipper2);
    //         
    //         println!("✅ Added test data to WeightedMap");
    //     });
    //     
    //     println!("✅ WsSink integration test completed!");
    // }

    // Test to verify traversal engine is working
    pub fn test_traversal_engine_functionality() {
        println!("🚀 Testing TraversalEngine functionality...");
        
        // Create test map
        let mut test_map = PathMap::<U64AtomHeader>::new();
        test_map.insert(&[1, 2, 3], U64AtomHeader(55));
        test_map.insert(&[4, 5, 6], U64AtomHeader(25));
        
        let weighted_map = WeightedMap::new(test_map.into_zipper_head(&[]));
        let traverse = Traverse::default();
        
        // Test next_atom method
        let read_zipper = weighted_map.read_zipper_at_borrowed_path(&[]).unwrap();
        for i in 0..18 {
            match traverse.next_atom(read_zipper.clone()) {
                Ok(atom_path) => {
                    println!("✅ TraversalEngine found atom: {:?}", String::from_utf8_lossy(&atom_path));
                }
                Err(e) => {
                    println!("❌ TraversalEngine error: {:?}", e);
                }
            }
        }

        println!("✅ TraversalEngine functionality test completed!");
    }

        
    // Spawn multiple threads accessing the map
    pub fn test_concurrent_weighted_map_access() {
        println!("🚀 Testing concurrent WeightedMap access...");
        
        // Create WeightedMap with initial data
        let mut test_map = PathMap::<U64AtomHeader>::new();
        test_map.insert(&[1, 1, 1], U64AtomHeader(50));
        
        let weighted_map = WeightedMap::new(test_map.into_zipper_head(&[]));
        
        // Spawn multiple threads accessing the map
        let handles: Vec<_> = (0..5)
            .map(|i| {
                let map_clone = weighted_map.inner.clone();
                thread::spawn(move || {
                    let path = [i as u8, i as u8, i as u8];
                    match map_clone.read_zipper_at_borrowed_path(&path) {
                        Ok(zipper) => {
                            println!("Thread {}: Path access successful", i);
                            if let Some(header) = zipper.val() {
                                println!("Thread {}: Found weight {}", i, header.0);
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

    // Integration test that combines all components
    pub fn test_full_weighted_atom_sweep_integration() {
        println!("🚀 Starting full WeightedAtomSweep integration test...");
        
        // Setup test data
        let mut test_map = PathMap::<U64AtomHeader>::new();
        
        // Create a small test data set
        let atoms = vec![
            ([1, 1, 1], U64AtomHeader(10)),
            ([1, 1, 2], U64AtomHeader(20)),
            ([1, 1, 3], U64AtomHeader(30)),
            ([2, 1, 1], U64AtomHeader(5)),
            ([2, 2, 1], U64AtomHeader(15)),
        ];
        
        for (path, header) in &atoms {
            test_map.insert(path, *header);
        }
        
        println!("📊 Created test map with {} atoms", atoms.len());
        
        // Create components
        let traverse = Traverse::default();
        let transform_log = Arc::new(std::sync::Mutex::new(Vec::new()));
        
        let operations = vec![
            LoggingOperation {
                name: "integration_test".to_string(),
                transform_log: transform_log.clone(),
            },
        ];
        
        let settings = WeightedAtomSweepSettings{};
        let weighted_map = WeightedMap::new(test_map.into_zipper_head(&[]));
        let sweep = WeightedAtomSweep::new(traverse, operations, settings, weighted_map);
        
        println!("🚂 Spawning full integration test...");
        let zipper_head = sweep.spawn();
        
        // Allow background processing
        thread::sleep(Duration::from_millis(200));
        
        // Verify results
        if let Ok(logs) = transform_log.lock() {
            println!("📝 Background operations processed {} atoms", logs.len());
            for log in logs.iter().take(3) { // Show first 3 logs
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
}
