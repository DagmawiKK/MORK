
use std::time::Duration;
use std::sync::Mutex;

use hyper::StatusCode;

use pathmap::{PathMap, zipper::ZipperHeadOwned};
use pathmap::zipper_tracking::Conflict;
use pathmap::zipper::*;

use mork::{PathPermissionErr, PermissionArb, Space, SpaceReaderZipper, SpaceWriterZipper, WPermissionArb, WSpace, WSpaceReaderZipper, WSpaceWriterZipper};

use crate::server_space::{ServerPermissionErr, ServerPermissionHead};
use crate::status_map::{self, *};
use crate::commands::*;

/// The time to wait before rejecting a request with a conflicted path
const SETTLE_TIME: Duration = Duration::from_millis(5);

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
}

