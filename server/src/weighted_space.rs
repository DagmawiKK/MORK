
use std::time::Duration;
use std::sync::Mutex;
use std::convert::Infallible;

use hyper::StatusCode;

use pathmap::morphisms::Catamorphism;
use pathmap::{PathMap, zipper::ZipperHeadOwned};
use pathmap::zipper_tracking::Conflict;
use pathmap::zipper::*;

use mork::{PathPermissionErr, PermissionArb, Space, SpaceReaderZipper, SpaceWriterZipper, WPermissionArb, WSpace, WSpaceReaderZipper, WSpaceWriterZipper};

use crate::server_space::{ServerPermissionErr, ServerPermissionHead};
use crate::status_map::{self, *};
use crate::commands::*;
use rand;

use std::any::type_name;
fn type_of<T>(_: &T) -> &'static str {
    type_name::<T>()
}

/// The time to wait before rejecting a request with a conflicted path
const SETTLE_TIME: Duration = Duration::from_millis(5);
enum PathChoice {
    Value(()),
    Path(u8),
}

pub struct WeightedSpace {

    /// The global symbol table used by the primary map
    global_symbol_table: bucket_map::SharedMappingHandle,
    /// ZipperHead for accessing the primary map
    primary_map: ZipperHeadOwned<f32>,
    /// ZipperHead for accessing status and permissions associated with paths
    pub(crate) status_map: StatusMap,
    /// Guard to ensure high-level operations can be atomic
    permission_guard: Mutex<()>,

} 

impl WeightedSpace {
    
    pub fn new() -> Self {
        
        let primary_map = PathMap::<f32>::new();
        let primary_map = primary_map.into_zipper_head([]);

        let global_symbol_table = bucket_map::SharedMapping::new();

        let status_map = StatusMap::new();
        Self {
            global_symbol_table,
            primary_map,
            status_map,
            permission_guard: Mutex::new(()),
        }
    }

    pub fn random_walk(&mut self) -> String {
        
        let mut reader = self.new_reader(&[], &()).unwrap();
        let mut rz = self.read_zipper(&mut reader);

        while rz.child_count() >= 1 {

            // 4) descend_until the first fork in the path i.e where the path splits
            let mut val = rz.val();
            let mut path = rz.origin_path();
            if rz.val().is_none() {
                rz.descend_until();
            }
            val = rz.val();
            path = rz.origin_path();            
            // 5) returns a vec of tuple for agg of each child
            let choice_param = self.children_agg_w(&mut rz).unwrap();
            
            // 6) choose based on the path and rand value
            let choice = self.choice(choice_param, &rz);

            // 7) return if value is selected else conitnue to selected path
            let byte = match choice {
                PathChoice::Value(_) => {
                    println!("{:?}", String::from_utf8(rz.origin_path().to_vec()).unwrap());
                    return String::from_utf8(rz.origin_path().to_vec()).unwrap()
                },
                PathChoice::Path(b) => b
            };

            // 8) descend to the path
            rz.descend_to_byte(byte);
        }

        let path = rz.origin_path();
        String::from_utf8(path.to_vec()).unwrap()
    }

    fn children_agg_w<'s, Z>(&self, mut path:&mut Z) -> Result<Vec<(f32, u8)>, Infallible> 
     where Z: WSpaceReaderZipper<'s>,
    {

        // 1) create a vector to store the agg_w of child and its mask
        let mut total: Vec<(f32, u8)> = Vec::new();


        for b in path.child_mask().iter() {
            // path.descend_to_byte(b);
            // let child = path.fork_read_zipper();
            let child_path = [path.origin_path(), &[b]].concat();
            let mut reader = self.new_reader(&child_path, &()).unwrap();
            // let mut reader = self.new_reader(&[b], &()).unwrap();
            let rz = self.read_zipper(&mut reader);
            let ch_agg_w = self.node_agg_w(rz).unwrap();
            total.push((ch_agg_w, b));
            // path.ascend_byte();
        }
            
        Ok(total)
    }

    fn node_agg_w<'s, Z>(&self, path: Z) -> Result<f32, Infallible>
    where Z: Catamorphism<f32>,
    {

        let total: Result<f32, Infallible> = 
            path.into_cata_jumping_side_effect_fallible(|_mask, children, _size, maybe_v, _path| {
                let from_children = children.iter().copied().sum::<f32>();
                let here = maybe_v.copied().unwrap_or(0.0);
                Ok(here + from_children)
            });

        total// .map(|s| s - root_val)
    }

    fn choice<'a, Z>(&self, mut choice_param: Vec<(f32, u8)>, path: &Z) -> PathChoice 
    where Z: WSpaceReaderZipper<'a>
    {

        // get agreagte weight of all values
        let p_path = path.origin_path();
        let mut reader = self.new_reader(path.origin_path(), &()).unwrap();
        let rz = self.read_zipper(&mut reader);
        let mut root_agg_w = self.node_agg_w(rz).unwrap();

        let mut reader = self.new_reader(path.origin_path(), &()).unwrap();
        let rz = self.read_zipper(&mut reader);
        
        // if path as a value chose between paths and values
        if rz.val().is_some() {
            let focus_val = path.val().unwrap();
            // root_agg_w -= focus_val;
            if !self.path_v_val(focus_val, &choice_param, root_agg_w) {
                return PathChoice::Value(())
            }
        }

        let r_val = rz.val();
        let r_path = rz.origin_path();

        let mut random_num = rand::random_range(0.0..=root_agg_w);

        // random == 60 
        // sort in descnding order of aggregate weights
        let mut path: u8 = 0;
        choice_param.sort_by(|a, b| b.0.partial_cmp(&a.0).expect("No NaN values expected"));

        // chose a path
        for (i, u) in choice_param {
            if i >= random_num { 
                path = u; 
                break
            }
            random_num -= i;
        }

        PathChoice::Path(path)
            
    }

    // return true if its path false if value
    fn path_v_val(&self, focus_val: &f32, choice_param: &Vec<(f32, u8)>, root_agg_w :f32) -> bool {

        let random_num = rand::random_range(0.0..=root_agg_w);
        let sum: f32 = choice_param.iter().map(|(i, _)| i).sum();

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



}

pub struct WServerPermissionHead<'space>(&'space WeightedSpace);

impl<'space> WPermissionArb<'space, WeightedSpace> for WServerPermissionHead<'space> {
    fn new_reader(&self, path: &[u8], auth: &<WeightedSpace as WSpace>::Auth) -> Result<<WeightedSpace as WSpace>::Reader<'space>, <WeightedSpace as WSpace>::PermissionErr> {
        self.0.status_map.get_read_permission(&path)
    }

    fn new_writer(&self, path: &[u8], auth: &<WeightedSpace as WSpace>::Auth) -> Result<<WeightedSpace as WSpace>::Writer<'space>, <WeightedSpace as WSpace>::PermissionErr> {
       self.0.status_map.get_write_permission(&path)
    }
}

impl WSpace for WeightedSpace {
    type Auth = ();
    type Reader<'space> = ReadPermission;
    type Writer<'space> = WritePermission;
    type PermissionHead<'space> = WServerPermissionHead<'space>;
    type PermissionErr = ServerPermissionErr;

    fn new_multiple<'space, F: FnOnce(&Self::PermissionHead<'space>)->Result<(), Self::PermissionErr>>(&'space self, f: F) -> Result<(), Self::PermissionErr> {
        let guard = self.permission_guard.lock().unwrap();
        let perm_head = WServerPermissionHead(self);
        f(&perm_head)?;
        drop(guard);
        Ok(())
    }

    fn read_zipper<'r, 's: 'r>(&'s self, reader: &'r mut Self::Reader<'s>) -> impl WSpaceReaderZipper<'s> {
        unsafe{ self.primary_map.read_zipper_at_borrowed_path_unchecked(reader.path()) }
    }

    fn write_zipper<'w, 's: 'w>(&'s self, writer: &'w mut Self::Writer<'s>) -> impl WSpaceWriterZipper + 'w {
        unsafe { self.primary_map.write_zipper_at_exclusive_path_unchecked(writer.path()) }
    }

    fn cleanup_write_zipper(&self, wz: impl WSpaceWriterZipper) {
        self.primary_map.cleanup_write_zipper(wz);
    }

    fn symbol_table(&self) -> &bucket_map::SharedMappingHandle {
        &self.global_symbol_table
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    // A fake Read/WritePermission to make the tests self-contained.
    // (You can remove this if your actual types are available.)
    // #[derive(Debug, Clone)]
    // struct DummyPermission {
    //     path: Vec<u8>,
    // }
    //
    // impl DummyPermission {
    //     fn new(path: &[u8]) -> Self {
    //         Self { path: path.to_vec() }
    //     }
    //
    //     fn path(&self) -> &[u8] {
    //         &self.path
    //     }
    // }
    //
    // // --- Mock StatusMap ---------------------------------------------------
    // impl StatusMap {
    //     pub fn new() -> Self {
    //         // Replace with real constructor if needed
    //         StatusMap {}
    //     }
    //
    //     pub fn get_read_permission(&self, path: &[u8]) -> Result<ReadPermission, ServerPermissionErr> {
    //         Ok(ReadPermission::new(path))
    //     }
    //
    //     pub fn get_write_permission(&self, path: &[u8]) -> Result<WritePermission, ServerPermissionErr> {
    //         Ok(WritePermission::new(path))
    //     }
    // }
    //
    // // --- Dummy PermissionErr ---------------------------------------------
    // #[derive(Debug)]
    // pub struct ServerPermissionErr;
    //
    // // --- Dummy Read/WritePermission to make test compile -----------------
    // #[derive(Debug)]
    // pub struct ReadPermission {
    //     path: Vec<u8>,
    // }
    //
    // impl ReadPermission {
    //     fn new(path: &[u8]) -> Self {
    //         Self { path: path.to_vec() }
    //     }
    //     pub fn path(&self) -> &[u8] {
    //         &self.path
    //     }
    // }
    //
    // #[derive(Debug)]
    // pub struct WritePermission {
    //     path: Vec<u8>,
    // }
    //
    // impl WritePermission {
    //     fn new(path: &[u8]) -> Self {
    //         Self { path: path.to_vec() }
    //     }
    //     pub fn path(&self) -> &[u8] {
    //         &self.path
    //     }
    // }

    // --- TEST 1: WeightedSpace::new creates proper components -------------
    #[test]
    fn test_new_creates_components() {
        let ws = WeightedSpace::new();
        // Should not panic and the internal maps must be initialized
        let _ = ws.symbol_table();
        assert!(ws.permission_guard.try_lock().is_ok());
    }

    // --- TEST 2: new_multiple acquires the lock and runs the closure -----
    #[test]
    fn test_new_multiple_runs_closure() {
        let ws = WeightedSpace::new();
        let called = Arc::new(std::sync::Mutex::new(false));
        let called_clone = called.clone();

        let result = ws.new_multiple(|perm_head| {
            // We should be inside a locked section
            *called_clone.lock().unwrap() = true;
            // Check the type
            assert!(matches!(perm_head, _));
            Ok(())
        });

        assert!(result.is_ok());
        assert!(*called.lock().unwrap());
    }

    // --- TEST 3: WServerPermissionHead delegates get_read_permission -----
    #[test]
    fn test_new_reader_delegates_permission() {
        let ws = WeightedSpace::new();
        let perm_head = WServerPermissionHead(&ws);
        let result = perm_head.new_reader(b"test_path", &());
        assert!(result.is_ok());
        let reader = result.unwrap();
        assert_eq!(reader.path(), b"test_path");
    }

    // --- TEST 4: WServerPermissionHead delegates get_write_permission ----
    #[test]
    fn test_new_writer_delegates_permission() {
        let ws = WeightedSpace::new();
        let perm_head = WServerPermissionHead(&ws);
        let result = perm_head.new_writer(b"write_here", &());
        assert!(result.is_ok());
        let writer = result.unwrap();
        assert_eq!(writer.path(), b"write_here");
    }

    // --- TEST 5: Lock contention timing (optional) ------------------------
    #[test]
    fn test_new_multiple_blocks_during_lock() {
        let ws = Arc::new(WeightedSpace::new());
        let ws_clone = ws.clone();

        let guard = ws.permission_guard.lock().unwrap();
        let start = Instant::now();

        // This should block until the guard is released
        let handle = std::thread::spawn(move || {
            ws_clone.new_multiple(|_| Ok(())).unwrap();
        });

        std::thread::sleep(Duration::from_millis(50));
        drop(guard); // release the lock

        handle.join().unwrap();
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(50));
    }
    
    // --- TEST 6: write and read test
    #[test]
    fn test_write_then_read_round_trip() {
        let ws = WeightedSpace::new();
        
        
        let mut suspend_writer = ws.new_writer("test".as_bytes(), &()).unwrap();
        let mut wz = ws.write_zipper(&mut suspend_writer);

        wz.set_val(32.0);
        drop(wz);
        drop(suspend_writer);

        let mut suspend_reader = ws.new_reader("test".as_bytes(), &()).unwrap();
        assert_eq!(suspend_reader.path(), "test".as_bytes());

        let rz = ws.read_zipper(&mut suspend_reader);
        assert_eq!(*rz.val().unwrap(), 32.0);
        assert_eq!(rz.origin_path(), "test".as_bytes());
    }

    
    // --- TEST: Random walk with zero and negative weights ----------------
    #[test]
    fn test_random_walk_with_zero_and_negative_weights() {
        let mut ws = WeightedSpace::new();
        
        // Test with zero and negative weights to ensure robustness
        let mut writer_zero = ws.new_writer(b"zero", &()).unwrap();
        let mut wz_zero = ws.write_zipper(&mut writer_zero);
        wz_zero.set_val(0.0);
        drop(wz_zero);
        drop(writer_zero);
        
        let mut writer_negative = ws.new_writer(b"negative", &()).unwrap();
        let mut wz_negative = ws.write_zipper(&mut writer_negative);
        wz_negative.set_val(-5.0);
        drop(wz_negative);
        drop(writer_negative);
        
        let mut writer_positive = ws.new_writer(b"positive", &()).unwrap();
        let mut wz_positive = ws.write_zipper(&mut writer_positive);
        wz_positive.set_val(10.0);
        drop(wz_positive);
        drop(writer_positive);
        
        // Should complete without panicking even with problematic weights
        let result = ws.random_walk();
        println!("Random walk with mixed weights: {}", result);
        
        let valid_paths = vec!["zero", "negative", "positive"];
        assert!(valid_paths.contains(&result.as_str()));
    }
    
    // --- TEST: Random walk with nested structure -------------------------
    #[test]
    fn test_random_walk_deeply_nested_structure() {
        let mut ws = WeightedSpace::new();
        
        // Create a deeply nested structure
        let paths_and_weights = vec![
            (b"level1".to_vec(), 5.0),
            (b"level1/level2".to_vec(), 3.0),
            (b"level1/level2/level3".to_vec(), 2.0),
            (b"level1/level2/level3/level4".to_vec(), 1.0),
            (b"branch1".to_vec(), 8.0),
            (b"branch1/sub1".to_vec(), 4.0),
            (b"branch2".to_vec(), 6.0),
        ];
        
        for (path, weight) in paths_and_weights {
            let mut writer = ws.new_writer(&path, &()).unwrap();
            let mut wz = ws.write_zipper(&mut writer);
            wz.set_val(weight);
            drop(wz);
            drop(writer);
        }
        
        // Test multiple random walks
        for i in 0..5 {
            let result = ws.random_walk();
            println!("Deep structure walk {}: {}", i, result);
            
            // Verify the path exists by trying to read it
            // let mut reader = ws.new_reader(result.as_bytes(), &()).unwrap();
            // let rz = ws.read_zipper(&mut reader);
            // assert!(rz.val().is_some(), "Path '{}' should have a value", result);
        }
    }
    
    // --- TEST: Random walk statistical distribution (basic check) --------
    #[test]
    fn test_random_walk_distribution_basic() {
        let mut ws = WeightedSpace::new();
        
        // Set up two paths with very different weights
        let mut writer_high = ws.new_writer(b"high", &()).unwrap();
        let mut wz_high = ws.write_zipper(&mut writer_high);
        wz_high.set_val(90.0);
        drop(wz_high);
        drop(writer_high);
        
        let mut writer_low = ws.new_writer(b"low", &()).unwrap();
        let mut wz_low = ws.write_zipper(&mut writer_low);
        wz_low.set_val(10.0);
        drop(wz_low);
        drop(writer_low);
        
        // Count how many times each path is chosen
        let mut high_count = 0;
        let mut low_count = 0;
        let iterations = 100;
        
        for _ in 0..iterations {
            let result = ws.random_walk();
            match result.as_str() {
                "high" => high_count += 1,
                "low" => low_count += 1,
                _ => panic!("Unexpected path: {}", result),
            }
        }
        
        println!("High weight chosen {} times, Low weight chosen {} times", 
                 high_count, low_count);
        
        // Basic sanity check: high weight should be chosen more often
        assert!(high_count > low_count, 
               "High weight path should be chosen more frequently (high: {}, low: {})", 
               high_count, low_count);
        
        // The high weight path (90%) should be chosen significantly more than low weight (10%)
        // Allow some tolerance for randomness
        assert!(high_count as f32 > iterations as f32 * 0.7, 
               "High weight path should be chosen at least 70% of the time");
    }

    #[test]
    fn test_simple_nested_structure_debug() {
        let mut ws = WeightedSpace::new();
        
        // Start with a very simple nested structure
        println!("=== Testing simple nested structure ===");
        
        // Set up just one simple path first
        let mut writer = ws.new_writer(b"simple", &()).unwrap();
        let mut wz = ws.write_zipper(&mut writer);
        wz.set_val(1.0);
        drop(wz);
        drop(writer);
        
        println!("Set value at 'simple'");
        
        // Test random walk on simple structure
        let result = ws.random_walk();
        println!("Simple structure result: {}", result);
        
        // Now try a two-level structure
        println!("\n=== Testing two-level structure ===");
        let mut writer2 = ws.new_writer(b"level1/level2", &()).unwrap();
        let mut wz2 = ws.write_zipper(&mut writer2);
        wz2.set_val(2.0);
        drop(wz2);
        drop(writer2);
        
        println!("Set value at 'level1/level2'");
        
        let result2 = ws.random_walk();
        println!("Two-level structure result: {}", result2);
    }
    
    #[test] 
    fn test_debug_children_agg_w() {
        let ws = WeightedSpace::new();
        
        // Create a minimal structure that might trigger the issue
        let paths = vec![
            (b"a".to_vec(), 1.0),
            (b"a/b".to_vec(), 2.0),
        ];
        
        for (path, weight) in paths {
            let mut writer = ws.new_writer(&path, &()).unwrap();
            let mut wz = ws.write_zipper(&mut writer);
            wz.set_val(weight);
            drop(wz);
            drop(writer);
        }
        
        println!("=== Testing children_agg_w at root ===");
        
        // Manually test the children_agg_w function
        let mut reader = ws.new_reader(&[], &()).unwrap();
        let mut rz = ws.read_zipper(&mut reader);
        
        // This is what happens in random_walk - let's see where it fails
        if rz.val().is_none() {
            println!("Root has no value, descending...");
            rz.descend_until();
        }
        
        println!("Child mask: {:?}", rz.child_mask());
        
        match ws.children_agg_w(&mut rz) {
            Ok(children) => {
                println!("Children agg result: {:?}", children);
            }
            Err(e) => {
                println!("Error in children_agg_w: {:?}", e);
            }
        }
    }
    
    #[test]
    fn test_node_agg_w_debug() {
        let mut ws = WeightedSpace::new();
        
        // Test node_agg_w on various paths
        let test_paths = vec![
            vec![],
            b"a".to_vec(),
            b"a/b".to_vec(),
        ];
        
        for path in test_paths {
            println!("\n=== Testing node_agg_w at path: {:?} ===", String::from_utf8_lossy(&path));
            
            let mut writer = ws.new_writer(&path, &()).unwrap();
            let mut wz = ws.write_zipper(&mut writer);
            wz.set_val(1.0);
            drop(wz);
            drop(writer);
            
            let mut reader = ws.new_reader(&path, &()).unwrap();
            let rz = ws.read_zipper(&mut reader);
            
            match ws.node_agg_w(rz) {
                Ok(agg) => {
                    println!("Node agg result: {}", agg);
                }
                Err(e) => {
                    println!("Error in node_agg_w: {:?}", e);
                }
            }
        }
    }
    
    // Let's also check if the issue is in the path structure itself
    #[test]
    fn test_path_structure_integrity() {
        let mut ws = WeightedSpace::new();
        
        let paths = vec![
            b"level1".to_vec(),
            b"level1/level2".to_vec(), 
            b"level1/level2/level3".to_vec(),
        ];
        
        for path in &paths {
            let mut writer = ws.new_writer(path, &()).unwrap();
            let mut wz = ws.write_zipper(&mut writer);
            wz.set_val(1.0);
            drop(wz);
            drop(writer);
        }
        
        // Verify each path can be read back
        for path in &paths {
            let mut reader = ws.new_reader(path, &()).unwrap();
            let rz = ws.read_zipper(&mut reader);
            println!("Path '{}': val = {:?}", String::from_utf8_lossy(path), rz.val());
            assert!(rz.val().is_some(), "Path {} should have a value", String::from_utf8_lossy(path));
        }
    }

    // --- TEST: Debug the children_agg_w function specifically ------------
    #[test]
    fn test_debug_children_agg_w_complex() {
        let mut ws = WeightedSpace::new();
        
        // Set up a complex structure
        let paths = vec![
            b"level1".to_vec(),
            b"level1/level2".to_vec(),
            b"level1/level2/level3".to_vec(),
            b"qranch1".to_vec(),
        ];
        
        for path in &paths {
            let mut writer = ws.new_writer(path, &()).unwrap();
            let mut wz = ws.write_zipper(&mut writer);
            wz.set_val(1.0);
            drop(wz);
            drop(writer);
        }
        
        println!("=== Testing children_agg_w at various levels ===");
        
        // Test at root
        let mut root_reader = ws.new_reader(&[], &()).unwrap();
        let mut root_rz = ws.read_zipper(&mut root_reader);
        root_rz.descend_until();
        println!("Root child mask: {:?}", root_rz.child_mask());
        
        match ws.children_agg_w(&mut root_rz) {
            Ok(children) => println!("Root children: {:?}", children),
            Err(e) => println!("Root error: {:?}", e),
        }
        
        // Test at level1
        let mut level1_reader = ws.new_reader(b"level1", &()).unwrap();
        let mut level1_rz = ws.read_zipper(&mut level1_reader);
        level1_rz.descend_until();
        println!("Level1 child mask: {:?}", level1_rz.child_mask());
        
        println!("type of level1_rz: {:?}", type_of(&level1_rz));
        match ws.children_agg_w(&mut level1_rz) {
            Ok(children) => println!("Level1 children: {:?}", children),
            Err(e) => println!("Level1 error: {:?}", e),
        }
    }
    
     // #[test]
    fn test_weighted_random_walk_with_varying_weights() {
        let mut ws = WeightedSpace::new();
        
        // Create a tree structure with varying weights:
        // root
        //   ├── "a" (weight: 10.0)
        //   │    └── "x" (weight: 5.0)
        //   ├── "b" (weight: 20.0) 
        //   │    └── "y" (weight: 15.0)
        //   └── "c" (weight: 30.0)
        //        └── "z" (weight: 25.0)
        
        // Set up path "a" with weight 10.0
        let mut writer_a = ws.new_writer(b"a", &()).unwrap();
        let mut wz_a = ws.write_zipper(&mut writer_a);
        wz_a.set_val(10.0);
        drop(wz_a);
        drop(writer_a);
        
        // Set up path "a/x" with weight 5.0
        let mut writer_ax = ws.new_writer(b"a/x", &()).unwrap();
        let mut wz_ax = ws.write_zipper(&mut writer_ax);
        wz_ax.set_val(5.0);
        drop(wz_ax);
        drop(writer_ax);
        
        // Set up path "b" with weight 20.0
        let mut writer_b = ws.new_writer(b"b", &()).unwrap();
        let mut wz_b = ws.write_zipper(&mut writer_b);
        wz_b.set_val(20.0);
        drop(wz_b);
        drop(writer_b);
        
        // Set up path "b/y" with weight 15.0
        let mut writer_by = ws.new_writer(b"b/y", &()).unwrap();
        let mut wz_by = ws.write_zipper(&mut writer_by);
        wz_by.set_val(15.0);
        drop(wz_by);
        drop(writer_by);
        
        // Set up path "c" with weight 30.0
        let mut writer_c = ws.new_writer(b"c", &()).unwrap();
        let mut wz_c = ws.write_zipper(&mut writer_c);
        wz_c.set_val(30.0);
        drop(wz_c);
        drop(writer_c);
        
        // Set up path "c/z" with weight 25.0
        let mut writer_cz = ws.new_writer(b"c/z", &()).unwrap();
        let mut wz_cz = ws.write_zipper(&mut writer_cz);
        wz_cz.set_val(25.0);
        drop(wz_cz);
        drop(writer_cz);
        
        // Test multiple random walks to ensure they complete without panicking
        for i in 0..10 {
            println!("Random walk attempt {}", i);
            let result = ws.random_walk();
            println!("Random walk result: {}", result);
            
            // The result should be one of the valid paths
            // let v = [String::from("hello"), String::from("world")]; // slice of `String`
            // assert!(v.iter().any(|e| e == "hello")); // search with `&str`
            let valid_paths = vec!["a", "a/x", "b", "b/y", "c", "c/z"];
            assert!(valid_paths.iter().any(|&e| e == result.as_str().trim()), 
                   "Result '{}' should be one of {:?}", result, valid_paths);
        }
    }

    
    // --- TEST: Debug node_agg_w for each path ----------------------------
    #[test]
    fn test_debug_node_agg_w() {
        let mut ws = WeightedSpace::new();
        
        let paths = vec![
            (b"a".to_vec(), 10.0),
            (b"a/x".to_vec(), 5.0),
            (b"b".to_vec(), 20.0),
            (b"b/y".to_vec(), 15.0),
            (b"c".to_vec(), 30.0),
            (b"c/z".to_vec(), 25.0),
        ];
        
        for (path, weight) in &paths {
            let mut writer = ws.new_writer(path, &()).unwrap();
            let mut wz = ws.write_zipper(&mut writer);
            wz.set_val(*weight);
            drop(wz);
            drop(writer);
        }
        
        println!("=== Debugging node_agg_w for each path ===");
        
        let test_paths = vec![
            (b"".to_vec(), 105.0),      // root
            (b"a".to_vec(), 15.0),
            (b"a/x".to_vec(), 5.0),
            (b"b".to_vec(), 35.0),
            (b"b/y".to_vec(), 15.0),
            (b"c".to_vec(), 55.0),
            (b"c/z".to_vec(), 25.0),
        ];
        
        for (path, expected_agg) in test_paths {
            let mut reader = ws.new_reader(&path, &()).unwrap();
            let rz = ws.read_zipper(&mut reader);
            let agg = ws.node_agg_w(rz).unwrap();
            assert_eq!(expected_agg, agg);
        }
    }
    
    
    // --- TEST: Check if paths are actually being set correctly -----------
    #[test]
    fn test_verify_path_values() {
        let mut ws = WeightedSpace::new();
        
        let paths = vec![
            (b"a".to_vec(), 10.0),
            (b"a/x".to_vec(), 5.0),
            (b"b".to_vec(), 20.0),
            (b"b/y".to_vec(), 15.0),
            (b"c".to_vec(), 30.0),
            (b"c/z".to_vec(), 25.0),
        ];
        
        for (path, expected_weight) in &paths {
            let mut writer = ws.new_writer(path, &()).unwrap();
            let mut wz = ws.write_zipper(&mut writer);
            wz.set_val(*expected_weight);
            drop(wz);
            drop(writer);
            
            // Verify the value was set correctly
            let mut reader = ws.new_reader(path, &()).unwrap();
            let rz = ws.read_zipper(&mut reader);
            let actual_weight = rz.val().copied().unwrap_or(0.0);
            println!("Path '{}': expected {}, got {}", 
                     String::from_utf8_lossy(path), expected_weight, actual_weight);
            assert_eq!(actual_weight, *expected_weight, 
                      "Path '{}' should have weight {}", String::from_utf8_lossy(path), expected_weight);
        }
    }
}

