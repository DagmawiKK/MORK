use std::cmp::Ordering;
use std::ptr::slice_from_raw_parts;

use crate::pure;
use crate::space::ACT_PATH;
use crate::weightedsweep::{GLOBAL_WS_SWEEP, Traverse, U64AtomHeader, init_weight};
use eval::EvalScope;
use eval_ffi::ExprSource;
use futures::StreamExt;
use log::*;
use mork_expr::macros::SerializableExpr;
use mork_expr::{
    Expr, ExprEnv, ExprZipper, ExtractFailure, Tag, UnificationFailure, apply, byte_item, destruct,
    item_byte, parse, serialize, traverseh, unify,
};
use mork_frontend::bytestring_parser::{Context, Parser, ParserError};
use mork_frontend::json_parser::Transcriber;
use mork_interning::{SharedMapping, SharedMappingHandle, WritePermit};
use pathmap::PathMap;
use pathmap::morphisms::Catamorphism;
use pathmap::ring::{AlgebraicStatus, Lattice};
use pathmap::utils::{BitMask, ByteMask};
use pathmap::zipper::*;
use weighted_atom_sweep::{AtomHeader, WeightedAtomSweep};

#[derive(Eq, PartialEq, Debug)]
pub(crate) enum WriteResourceRequest {
    BTM(&'static [u8]),
    ACT(&'static str),
    Z3(&'static str),
}

impl WriteResourceRequest {
    pub(crate) fn pjoin(&self, other: &Self) -> Option<Self> {
        match self {
            WriteResourceRequest::BTM(s) => match other {
                WriteResourceRequest::BTM(o) => Some(WriteResourceRequest::BTM(
                    &s[..pathmap::utils::find_prefix_overlap(s, o)],
                )),
                _ => None,
            },
            WriteResourceRequest::ACT(s) => match other {
                WriteResourceRequest::ACT(o) if s == o => Some(WriteResourceRequest::ACT(s)),
                _ => None,
            },
            WriteResourceRequest::Z3(_) => {
                unreachable!()
            }
        }
    }
}

impl PartialOrd for WriteResourceRequest {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match self {
            WriteResourceRequest::BTM(s) => {
                if let WriteResourceRequest::BTM(o) = other {
                    s.partial_cmp(o)
                } else {
                    None
                }
            }
            WriteResourceRequest::ACT(s) => {
                if let WriteResourceRequest::ACT(o) = other {
                    if s == o { Some(Ordering::Equal) } else { None }
                } else {
                    None
                }
            }
            WriteResourceRequest::Z3(_) => {
                unreachable!()
            }
        }
    }
}

pub(crate) enum WriteResource<'w, 'a, 'k, H>
where
    H: AtomHeader + Default,
{
    BTM(&'w mut WriteZipperTracked<'a, 'k, H>),
    ACT(()),
    Z3(&'w mut subprocess::Popen),
}

// trait JoinLattice  {
//     fn join(x: Self, y: Self) -> Self;
// }
//
// impl JoinLattice for WriteResourceRequest {
//     fn join(x: Self, y: Self) -> Self {
//         match (x, y) {
//             (WriteResourceRequest::BTM(x), WriteResourceRequest::BTM(y)) => {
//                 let i = pathmap::utils::find_prefix_overlap(x, y);
//                 &x[..i] // equiv &y[..i]
//             }
//         }
//     }
// }
//
// impl std::cmp::PartialEq for JoinLattice {
//     fn eq(&self, other: &Self) -> bool {
//         Self::is_bottom(self.meet(other))
//     }
//
// }
//
// impl std::cmp::PartialOrd for JoinLattice {
//     fn lteq(x: Self, y: Self) -> bool {
//         x.join(y).eq(y)
//     }
// }

pub trait Sink<H>
where
    H: AtomHeader + Default,
{
    fn new(e: Expr) -> Self;
    fn request(&self) -> impl Iterator<Item = WriteResourceRequest>;
    fn sink<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        it: It,
        path: &[u8],
    ) where
        'a: 'w,
        'k: 'w;
    fn finalize<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        it: It,
    ) -> bool
    where
        'a: 'w,
        'k: 'w;
}

pub struct CompatSink {
    e: Expr,
    changed: bool,
}

impl<H> Sink<H> for CompatSink
where
    H: AtomHeader + Default,
{
    fn new(e: Expr) -> Self {
        CompatSink { e, changed: false }
    }
    fn request(&self) -> impl Iterator<Item = WriteResourceRequest> {
        let p = &unsafe {
            self.e
                .prefix()
                .unwrap_or_else(|x| self.e.span())
                .as_ref()
                .unwrap()
        }[..];
        trace!(target: "sink", "+ (compat) requesting {}", serialize(p));
        std::iter::once(WriteResourceRequest::BTM(p))
    }
    fn sink<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
        path: &[u8],
    ) where
        'a: 'w,
        'k: 'w,
    {
        let WriteResource::BTM(wz) = it.next().unwrap() else {
            unreachable!()
        };
        let mpath = &path[wz.root_prefix_path().len()..];
        trace!(target: "sink", "+ (compat) at '{}' sinking raw '{}'", serialize(wz.root_prefix_path()), serialize(path));
        trace!(target: "sink", "+ (compat) sinking '{}'", serialize(mpath));
        wz.move_to_path(mpath);
        self.changed |= wz.set_val(H::default()).is_none();
    }
    fn finalize<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        it: It,
    ) -> bool
    where
        'a: 'w,
        'k: 'w,
    {
        trace!(target: "sink", "+ (compat) finalizing");
        self.changed
    }
}

pub struct AddSink {
    e: Expr,
    changed: bool,
}
impl<H> Sink<H> for AddSink
where
    H: AtomHeader + Default,
{
    fn new(e: Expr) -> Self {
        AddSink { e, changed: false }
    }
    fn request(&self) -> impl Iterator<Item = WriteResourceRequest> {
        let p = &unsafe {
            self.e
                .prefix()
                .unwrap_or_else(|x| self.e.span())
                .as_ref()
                .unwrap()
        }[3..];
        trace!(target: "sink", "+ requesting {}", serialize(p));
        std::iter::once(WriteResourceRequest::BTM(p))
    }
    fn sink<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
        path: &[u8],
    ) where
        'a: 'w,
        'k: 'w,
    {
        let WriteResource::BTM(wz) = it.next().unwrap() else {
            unreachable!()
        };
        let mpath = &path[3 + wz.root_prefix_path().len()..];
        trace!(target: "sink", "+ at '{}' sinking raw '{}'", serialize(wz.root_prefix_path()), serialize(path));
        trace!(target: "sink", "+ sinking '{}'", serialize(mpath));
        wz.move_to_path(mpath);
        self.changed |= wz.set_val(H::default()).is_none();
    }
    fn finalize<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        it: It,
    ) -> bool
    where
        'a: 'w,
        'k: 'w,
    {
        trace!(target: "sink", "+ finalizing");
        self.changed
    }
}

// (U <expr>)
pub struct USink {
    e: Expr,
    buf: Option<*mut u8>,
    tmp: Option<*mut u8>,
    last: usize,
    conflict: bool,
}
impl<H> Sink<H> for USink
where
    H: AtomHeader + Default,
{
    fn new(e: Expr) -> Self {
        USink {
            e,
            buf: None,
            tmp: None,
            last: usize::MAX,
            conflict: false,
        }
    }
    fn request(&self) -> impl Iterator<Item = WriteResourceRequest> {
        let p = &unsafe {
            self.e
                .prefix()
                .unwrap_or_else(|x| self.e.span())
                .as_ref()
                .unwrap()
        }[3..];
        trace!(target: "sink", "U requesting {}", serialize(p));
        std::iter::once(WriteResourceRequest::BTM(p))
    }
    fn sink<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
        path: &[u8],
    ) where
        'a: 'w,
        'k: 'w,
    {
        // we could be way more parsimonious not unifying the prefix over and over again
        // let mpath = &path[3+wz.root_prefix_path().len()..];
        trace!(target: "sink", "U new expr '{}'", serialize(&path[3..]));
        if self.conflict {
            return;
        }
        if let Some(e) = self.buf {
            let mut tmp = self.tmp.unwrap();
            let eau = Expr { ptr: e };
            let mut wz = ExprZipper::new(Expr { ptr: tmp });
            let Ok(_) = eau.unify(
                Expr {
                    ptr: path[3..].as_ptr().cast_mut(),
                },
                &mut wz,
            ) else {
                self.conflict = true;
                return;
            };
            std::mem::swap(&mut self.buf, &mut self.tmp);
            self.last = wz.loc;
        } else {
            self.buf = Some(unsafe {
                std::alloc::alloc(std::alloc::Layout::array::<u8>(1 << 32).unwrap())
            });
            self.tmp = Some(unsafe {
                std::alloc::alloc(std::alloc::Layout::array::<u8>(1 << 32).unwrap())
            });
            unsafe {
                std::ptr::copy_nonoverlapping(
                    path[3..].as_ptr(),
                    self.buf.unwrap(),
                    path[3..].len(),
                )
            }
            self.last = path[3..].len();
        }
    }
    fn finalize<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
    ) -> bool
    where
        'a: 'w,
        'k: 'w,
    {
        trace!(target: "sink", "U finalizing");
        if self.conflict {
            trace!(target: "sink", "U conflict");
            return false;
        }
        match self.buf.take() {
            None => {
                trace!(target: "sink", "U empty");
                false
            }
            Some(buf) => {
                let buf = unsafe {
                    slice_from_raw_parts(buf as *const u8, self.last)
                        .as_ref()
                        .unwrap()
                };
                trace!(target: "sink", "U unified expression '{}'", serialize(buf));
                let WriteResource::BTM(wz) = it.next().unwrap() else {
                    unreachable!()
                };
                wz.move_to_path(&buf[wz.root_prefix_path().len()..]);
                wz.set_val(H::default());
                true
            }
        }
    }
}

// (AU <expr>)
pub struct AUSink {
    e: Expr,
    buf: Option<Box<[u8]>>,
    tmp: Option<Box<[u8]>>,
    last: usize,
}
impl<H> Sink<H> for AUSink
where
    H: AtomHeader + Default,
{
    fn new(e: Expr) -> Self {
        AUSink {
            e,
            buf: None,
            tmp: None,
            last: usize::MAX,
        }
    }
    fn request(&self) -> impl Iterator<Item = WriteResourceRequest> {
        let p = &unsafe {
            self.e
                .prefix()
                .unwrap_or_else(|x| self.e.span())
                .as_ref()
                .unwrap()
        }[4..];
        trace!(target: "sink", "AU requesting {}", serialize(p));
        std::iter::once(WriteResourceRequest::BTM(p))
    }
    fn sink<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
        path: &[u8],
    ) where
        'a: 'w,
        'k: 'w,
    {
        // we could be way more parsimonious not anti-unifying the prefix over and over again
        // let mpath = &path[4+wz.root_prefix_path().len()..];
        trace!(target: "sink", "AU new expr '{}'", serialize(&path[4..]));
        if let Some(mut e) = self.buf.as_mut() {
            let mut tmp = self.tmp.as_mut().unwrap();
            let eau = Expr {
                ptr: (*e).as_mut_ptr(),
            };
            let mut wz = ExprZipper::new(Expr {
                ptr: (*tmp).as_mut_ptr(),
            });
            eau.anti_unify(
                Expr {
                    ptr: path[4..].as_ptr().cast_mut(),
                },
                &mut wz,
            )
            .unwrap();
            std::mem::swap(&mut self.buf, &mut self.tmp);
            self.last = wz.loc;
        } else {
            self.buf = Some(path[4..].to_vec().into_boxed_slice());
            self.tmp = self.buf.clone();
        }
    }
    fn finalize<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
    ) -> bool
    where
        'a: 'w,
        'k: 'w,
    {
        trace!(target: "sink", "AU finalizing");
        match self.buf.take() {
            None => {
                trace!(target: "sink", "AU empty");
                false
            }
            Some(buf) => {
                trace!(target: "sink", "AU anti-unified expression '{}'", serialize(&buf[..self.last]));
                let WriteResource::BTM(wz) = it.next().unwrap() else {
                    unreachable!()
                };
                wz.move_to_path(&buf[wz.root_prefix_path().len()..self.last]);
                wz.set_val(H::default());
                true
            }
        }
    }
}

pub struct ACTSink {
    e: Expr,
    file: &'static str,
    tmp: PathMap<()>,
}
impl<H> Sink<H> for ACTSink
where
    H: AtomHeader + Default,
{
    fn new(e: Expr) -> Self {
        destruct!(e, ("ACT" {act: &str} se), {
            return ACTSink { e, file: act, tmp: PathMap::new() }
        }, _err => { panic!("act not the right shape") });
    }
    fn request(&self) -> impl Iterator<Item = WriteResourceRequest> {
        trace!(target: "sink", "ACT requesting {}", self.file);
        std::iter::once(WriteResourceRequest::ACT(self.file))
    }
    fn sink<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
        path: &[u8],
    ) where
        'a: 'w,
        'k: 'w,
    {
        trace!(target: "sink", "ACT sinking '{}'", serialize(&path[1+1+3+self.file.len()..]));
        self.tmp
            .insert(&path[1 + 1 + 3 + 1 + self.file.len()..], ());
    }
    fn finalize<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
    ) -> bool
    where
        'a: 'w,
        'k: 'w,
    {
        trace!(target: "sink", "ACT finalizing");
        let _ = it.next().unwrap() else {
            unreachable!()
        };
        pathmap::arena_compact::ArenaCompactTree::dump_from_zipper(
            self.tmp.read_zipper(),
            |_v| 0,
            format!("{}{}.act", ACT_PATH, self.file),
        )
        .map(|_tree| ());
        true
    }
}

// pub struct RemoveSink { e: Expr, remove: PathMap<()> }
// // perhaps more performant to graft, remove*, and graft back?
// impl<H> Sink<H> for RemoveSink
// where
//     H: AtomHeader + Default
// {
//     fn new(e: Expr) -> Self { RemoveSink { e, remove: PathMap::new() } }
//     fn request(&self) ->  impl Iterator<Item=WriteResourceRequest> {
//         // !! we're never grabbing the full expression path, because then we don't have the ability to remove the root value
//         let p = &unsafe { self.e.prefix().unwrap_or_else(|x| { let s = self.e.span(); slice_from_raw_parts(self.e.ptr, s.len() - 1) }).as_ref().unwrap() }[3..];
//         trace!(target: "sink", "- requesting {}", serialize(p));
//         std::iter::once(WriteResourceRequest::BTM(p))
//     }
//     fn sink<'w, 'a, 'k, It : Iterator<Item=WriteResource<'w, 'a, 'k, H>>>(&mut self, mut it: It, path: &[u8]) where 'a : 'w, 'k : 'w {
//         let WriteResource::BTM(wz) = it.next().unwrap() else { unreachable!() };
//         let mpath = &path[3+wz.root_prefix_path().len()..];
//         trace!(target: "sink", "- at '{}' sinking raw '{}'", serialize(wz.root_prefix_path()), serialize(path));
//         trace!(target: "sink", "- sinking '{}'", serialize(mpath));
//         self.remove.insert(mpath, ());
//     }
//     fn finalize<'w, 'a, 'k, It : Iterator<Item=WriteResource<'w, 'a, 'k, H>>>(&mut self, mut it: It) -> bool where 'a : 'w, 'k : 'w {
//         let WriteResource::BTM(wz) = it.next().unwrap() else { unreachable!() };
//         wz.reset();
//         trace!(target: "sink", "- finalizing by subtracting {} at '{}'", self.remove.val_count(), serialize(wz.origin_path()));
//         // match self.remove.remove(&[]) {
//         //     None => {}
//         //     Some(s) => {
//         //         println!("has root");
//         //         wz.remove_val(true);
//         //         println!("val not removed");
//         //     }
//         // }
//         match wz.subtract_into(&self.remove.read_zipper(), true) {
//             AlgebraicStatus::Element => { true }
//             AlgebraicStatus::Identity => { false }
//             AlgebraicStatus::None => { true } // GOAT maybe not?
//         }
//     }
// }

// pub struct HeadSink { e: Expr, head: PathMap<()>, skip: usize, count: usize, max: usize, top: Vec<u8> }
// impl<H> Sink<H> for HeadSink
// where
//     H: AtomHeader + Default
// {
//     fn new(e: Expr) -> Self {
//         let mut ez = ExprZipper::new(e); ez.next(); ez.next();
//         let max_s = ez.item().err().expect("cnt can not be an expression or variable");
//         let max: usize = str::from_utf8(max_s).expect("string encoded numbers for now").parse().expect("a number");
//         assert_ne!(max, 0);
//         HeadSink { e, head: PathMap::new(), skip: 1 + 1+4 + 1+max_s.len(), count: 0, max, top: vec![] }
//     }
//     fn request(&self) ->  impl Iterator<Item=WriteResourceRequest> {
//         let p = &unsafe { self.e.prefix().unwrap_or_else(|x| { let s = self.e.span(); slice_from_raw_parts(self.e.ptr, s.len() - 1) }).as_ref().unwrap() }[self.skip..];
//         trace!(target: "sink", "head requesting {}", serialize(p));
//         std::iter::once(WriteResourceRequest::BTM(p))
//     }
//     fn sink<'w, 'a, 'k, It : Iterator<Item=WriteResource<'w, 'a, 'k, H>>>(&mut self, mut it: It, path: &[u8]) where 'a : 'w, 'k : 'w {
//         let WriteResource::BTM(wz) = it.next().unwrap() else { unreachable!() };
//         let mpath = &path[self.skip+wz.root_prefix_path().len()..];
//         trace!(target: "sink", "head at '{}' sinking raw '{}'", serialize(wz.root_prefix_path()), serialize(path));
//         if self.count == self.max {
//             if &self.top[..] <= mpath {
//                 trace!(target: "sink", "head at max capacity ignoring '{}'", serialize(mpath));
//                 // doesn't displace any path
//             } else {
//                 trace!(target: "sink", "head at max capacity replacing '{}' with '{}'", serialize(&self.top[..]), serialize(mpath));
//                 assert!(self.head.insert(mpath, ()).is_none());
//                 self.head.remove(&self.top[..]);
//                 let mut rz = self.head.read_zipper();
//                 rz.descend_last_path();
//                 self.top.clear();
//                 self.top.extend_from_slice(rz.path()); // yikes, throwing away our needless allocation
//             }
//         } else {
//             if &self.top[..] <= mpath {
//                 if self.head.insert(mpath, ()).is_none() {
//                     trace!(target: "sink", "head adding new top at '{}'", serialize(mpath));
//                     self.top.clear();
//                     self.top.extend_from_slice(mpath);
//                     self.count += 1;
//                 }
//             } else {
//                 if self.head.insert(mpath, ()).is_none() {
//                     trace!(target: "sink", "head adding '{}'", serialize(mpath));
//                     self.count += 1;
//                 }
//             }
//         }
//     }
//     fn finalize<'w, 'a, 'k, It : Iterator<Item=WriteResource<'w, 'a, 'k, H>>>(&mut self, mut it: It) -> bool where 'a : 'w, 'k : 'w {
//         let WriteResource::BTM(wz) = it.next().unwrap() else { unreachable!() };
//         wz.reset();
//         trace!(target: "sink", "head finalizing by joining {} at '{}'", self.count, serialize(wz.origin_path()));
//
//         match wz.join_into(&self.head.read_zipper()) {
//             AlgebraicStatus::Element => { true }
//             AlgebraicStatus::Identity => { false }
//             AlgebraicStatus::None => { true } // GOAT maybe not?
//         }
//     }
// }

#[cfg(feature = "wasm")]
pub struct WASMSink {
    e: Expr,
    skip: usize,
    changed: bool,
    module: wasmtime::Module,
    store: wasmtime::Store<()>,
    instance: wasmtime::Instance,
}

#[cfg(feature = "wasm")]
static ENGINE_LINKER: LazyLock<(wasmtime::Engine, wasmtime::Linker<()>)> = LazyLock::new(|| {
    let mut config = wasmtime::Config::new();
    config.wasm_multi_memory(true);
    config.strategy(wasmtime::Strategy::Cranelift);
    config.signals_based_traps(true);
    config.memory_reservation(1 << 32);
    config.memory_guard_size(1 << 32);
    #[cfg(all(target_feature = "avx2"))]
    unsafe {
        config.cranelift_flag_enable("has_sse3");
        config.cranelift_flag_enable("has_ssse3");
        config.cranelift_flag_enable("has_sse41");
        config.cranelift_flag_enable("has_sse42");
        config.cranelift_flag_enable("has_avx");
        config.cranelift_flag_enable("has_avx2");
        config.cranelift_flag_enable("has_bmi1");
        config.cranelift_flag_enable("has_bmi2");
        config.cranelift_flag_enable("has_lzcnt");
        config.cranelift_flag_enable("has_popcnt");
        config.cranelift_flag_enable("has_fma");
    }
    #[cfg(all(target_feature = "avx512"))]
    unsafe {
        config.cranelift_flag_enable("has_avx512bitalg");
        config.cranelift_flag_enable("has_avx512dq");
        config.cranelift_flag_enable("has_avx512vl");
        config.cranelift_flag_enable("has_avx512vbmi");
        config.cranelift_flag_enable("has_avx512f");
    }

    let engine = wasmtime::Engine::new(&config).unwrap();

    let mut linker = wasmtime::Linker::new(&engine);

    linker
        .func_wrap("", "i32.bswap", |param: i32| param.to_be())
        .unwrap();
    linker
        .func_wrap("", "i64.bswap", |param: i64| param.to_be())
        .unwrap();

    (engine, linker)
});

#[cfg(feature = "wasm")]
static mut LINKER: Option<wasmtime::Linker<()>> = None;
macro_rules! wasm_ctx {
    () => {
        r#"
(module
  (import "" "i32.bswap" (func $i32.bswap (param i32) (result i32)))
  (import "" "i64.bswap" (func $i64.bswap (param i64) (result i64)))

  (memory $in 1)
  (export "in" (memory $in))
  (memory $out 1)
  (export "out" (memory $out))
  (memory $local 1)

  (func (export "_otf_grounding")
    {:?}
  )
)
"#
    };
}

#[cfg(feature = "wasm")]
impl Sink for WASMSink {
    fn new(e: Expr) -> Self {
        let mut ez = ExprZipper::new(e);
        ez.next();
        ez.next();
        let program_e = ez.subexpr();
        let wat = format!(wasm_ctx!(), program_e);
        let module = wasmtime::Module::new(&ENGINE_LINKER.0, wat).unwrap();
        let mut store = wasmtime::Store::new(&ENGINE_LINKER.0, ());
        let instance = (&ENGINE_LINKER.1).instantiate(&mut store, &module).unwrap();

        WASMSink {
            e,
            skip: 1 + 1 + 4 + program_e.span().len(),
            changed: false,
            module,
            store,
            instance,
        }
    }
    fn request(&self) -> impl Iterator<Item = &'static [u8]> {
        // let p = &unsafe { self.e.prefix().unwrap_or_else(|x| { let s = self.e.span(); slice_from_raw_parts(self.e.ptr, s.len() - 1) }).as_ref().unwrap() }[self.skip..];
        // trace!(target: "sink", "wasm requesting {}", serialize(p));
        // std::iter::once(p)
        static empty: [u8; 0] = [];
        std::iter::once(&empty[..])
    }
    fn sink<'w, 'a, 'k, It: Iterator<Item = &'w mut WriteZipperUntracked<'a, 'k, ()>>>(
        &mut self,
        mut it: It,
        path: &[u8],
    ) where
        'a: 'w,
        'k: 'w,
    {
        let mut wz = it.next().unwrap();
        let mpath = &path[self.skip + wz.root_prefix_path().len()..];
        trace!(target: "sink", "wasm at '{}' sinking raw '{}'", serialize(wz.root_prefix_path()), serialize(path));
        trace!(target: "sink", "wasm input '{}'", serialize(mpath));
        let imem = self.instance.get_memory(&mut self.store, "in").unwrap();
        imem.write(&mut self.store, 0, mpath).unwrap();
        let run = self
            .instance
            .get_typed_func::<(), ()>(&mut self.store, "_otf_grounding")
            .unwrap();
        match run.call(&mut self.store, ()) {
            Ok(()) => {
                let omem = self
                    .instance
                    .get_memory(&mut self.store, "out")
                    .unwrap()
                    .data(&mut self.store);
                let ospan = unsafe {
                    Expr {
                        ptr: omem.as_ptr().cast_mut(),
                    }
                    .span()
                    .as_ref()
                    .unwrap()
                };
                trace!(target: "sink", "wasm output '{}'", serialize(ospan));
                wz.move_to_path(ospan);
                self.changed |= wz.set_val(()).is_none();
            }
            Err(e) => {
                trace!(target: "sink", "wasm error {:?}", e);
            }
        }
    }
    fn finalize<'w, 'a, 'k, It: Iterator<Item = &'w mut WriteZipperUntracked<'a, 'k, ()>>>(
        &mut self,
        mut it: It,
    ) -> bool
    where
        'a: 'w,
        'k: 'w,
    {
        trace!(target: "sink", "wasm finalizing");
        self.changed
    }
}

// ($k $x) (f $x $y)
// (count (count of $k is $i) $i ($x $y))   unify
// (count (count of r2 is $i) $i (P Q))
// (count (count of r2 is 3) 3 ($x $y))
pub struct CountSink {
    e: Expr,
    unique: PathMap<()>,
}
impl<H> Sink<H> for CountSink
where
    H: AtomHeader + Default,
{
    fn new(e: Expr) -> Self {
        CountSink {
            e,
            unique: PathMap::new(),
        }
    }
    fn request(&self) -> impl Iterator<Item = WriteResourceRequest> {
        let p = &unsafe {
            self.e
                .prefix()
                .unwrap_or_else(|x| {
                    let s = self.e.span();
                    slice_from_raw_parts(self.e.ptr, s.len() - 1)
                })
                .as_ref()
                .unwrap()
        }[7..];
        trace!(target: "sink", "count requesting {}", serialize(p));
        std::iter::once(WriteResourceRequest::BTM(p))
    }
    fn sink<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
        path: &[u8],
    ) where
        'a: 'w,
        'k: 'w,
    {
        let WriteResource::BTM(wz) = it.next().unwrap() else {
            unreachable!()
        };
        let mpath = &path[7 + wz.root_prefix_path().len()..];
        let ctx = unsafe {
            Expr {
                ptr: mpath.as_ptr().cast_mut(),
            }
        };
        trace!(target: "sink", "count at '{}' sinking raw '{}'", serialize(wz.root_prefix_path()), serialize(path));
        trace!(target: "sink", "count registering in ctx {:?}", serialize(mpath));
        self.unique.insert(mpath, ());
    }
    fn finalize<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
    ) -> bool
    where
        'a: 'w,
        'k: 'w,
    {
        let WriteResource::BTM(wz) = it.next().unwrap() else {
            unreachable!()
        };
        wz.reset();
        trace!(target: "sink", "count finalizing by reducing {} at '{}'", self.unique.val_count(), serialize(wz.origin_path()));

        let mut _to_swap = PathMap::new();
        std::mem::swap(&mut self.unique, &mut _to_swap);
        let mut rooted_input = PathMap::new();
        rooted_input
            .write_zipper_at_path(wz.root_prefix_path())
            .graft_map(_to_swap);

        static v: &'static [u8] = &[item_byte(Tag::NewVar)];
        let mut prz = OneFactor::new(rooted_input.into_read_zipper(&[]));
        let prz_ptr = (&prz) as *const OneFactor<_>;
        let mut changed = false;
        let mut buffer: Vec<u8> = Vec::with_capacity(1 << 32);
        crate::space::Space::<H>::query_multi_raw(
            unsafe { prz_ptr.cast_mut().as_mut().unwrap() },
            &[ExprEnv::new(
                0,
                Expr {
                    ptr: v.as_ptr().cast_mut(),
                },
            )],
            |refs_bindings, loc| {
                let cnt = prz.val_count();
                trace!(target: "sink", "'{}' and under {}", serialize(prz.path()), cnt);
                let clen = prz.path().len();
                let cnt_str = cnt.to_string();
                if prz.descend_to_existing_byte(item_byte(Tag::SymbolSize(cnt_str.len() as _))) {
                    let descended = prz.descend_to_existing(cnt_str.as_bytes());
                    if descended == cnt_str.len() {
                        let fixed = &prz.path()[..prz.path().len() - (1 + cnt_str.len())];
                        trace!(target: "sink", "fixed guard {}", serialize(fixed));
                        wz.move_to_path(fixed);
                        wz.set_val(H::default());
                        changed |= true;
                    }
                    prz.ascend(descended + 1);
                }
                if prz.descend_to_existing_byte(item_byte(Tag::NewVar)) {
                    let ignored = &prz.path()[..prz.path().len() - 1];
                    trace!(target: "sink", "ignored guard {}", serialize(ignored));
                    wz.move_to_path(ignored);
                    wz.set_val(H::default());
                    changed |= true;
                    prz.ascend_byte();
                }
                if prz.descend_first_byte() {
                    if let Tag::VarRef(k) = byte_item(prz.path()[prz.path().len() - 1]) {
                        let mut cntv = vec![item_byte(Tag::SymbolSize(cnt_str.len() as _))];
                        cntv.extend_from_slice(cnt_str.as_bytes());
                        let varref = &prz.path()[..prz.path().len() - 1];
                        let ie = Expr {
                            ptr: (&varref[0] as *const u8).cast_mut(),
                        };
                        let mut oz = ExprZipper::new(Expr {
                            ptr: buffer.as_mut_ptr(),
                        });
                        trace!(target: "sink", "ref guard '{}' var {:?} with '{}'", serialize(varref), k, serialize(&cntv[..]));
                        let os = ie.substitute_one_de_bruijn(
                            k,
                            Expr {
                                ptr: cntv.as_mut_ptr(),
                            },
                            &mut oz,
                        );
                        unsafe { buffer.set_len(oz.loc) }
                        trace!(target: "sink", "ref guard subs '{:?}'", serialize(&buffer[..oz.loc]));
                        wz.move_to_path(&buffer[wz.root_prefix_path().len()..oz.loc]);
                        wz.set_val(H::default());
                        changed |= true
                    }
                    prz.ascend_byte();
                }
                true
            },
        );
        changed
    }
}

pub struct HashSink {
    e: Expr,
    unique: PathMap<()>,
}
impl<H> Sink<H> for HashSink
where
    H: AtomHeader + Default,
{
    fn new(e: Expr) -> Self {
        Self {
            e,
            unique: PathMap::new(),
        }
    }
    fn request(&self) -> impl Iterator<Item = WriteResourceRequest> {
        let p = &unsafe {
            self.e
                .prefix()
                .unwrap_or_else(|x| {
                    let s = self.e.span();
                    slice_from_raw_parts(self.e.ptr, s.len() - 1)
                })
                .as_ref()
                .unwrap()
        }[6..];
        trace!(target: "sink", "hash requesting {}", serialize(p));
        std::iter::once(WriteResourceRequest::BTM(p))
    }
    fn sink<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
        path: &[u8],
    ) where
        'a: 'w,
        'k: 'w,
    {
        let WriteResource::BTM(wz) = it.next().unwrap() else {
            unreachable!()
        };
        let mpath = &path[6 + wz.root_prefix_path().len()..];
        let ctx = unsafe {
            Expr {
                ptr: mpath.as_ptr().cast_mut(),
            }
        };
        trace!(target: "sink", "hash at '{}' sinking raw '{}'", serialize(wz.root_prefix_path()), serialize(path));
        trace!(target: "sink", "hash registering in ctx {:?}", serialize(mpath));
        self.unique.insert(mpath, ());
    }
    fn finalize<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
    ) -> bool
    where
        'a: 'w,
        'k: 'w,
    {
        let WriteResource::BTM(wz) = it.next().unwrap() else {
            unreachable!()
        };
        wz.reset();
        trace!(target: "sink", "hash finalizing by reducing {} at '{}'", self.unique.val_count(), serialize(wz.origin_path()));

        let mut _to_swap = PathMap::new();
        std::mem::swap(&mut self.unique, &mut _to_swap);
        let mut rooted_input = PathMap::new();
        rooted_input
            .write_zipper_at_path(wz.root_prefix_path())
            .graft_map(_to_swap);

        static v: &'static [u8] = &[item_byte(Tag::NewVar)];
        let mut prz = OneFactor::new(rooted_input.into_read_zipper(&[]));
        let prz_ptr = (&prz) as *const OneFactor<_>;
        let mut changed = false;
        let mut buffer: Vec<u8> = Vec::with_capacity(1 << 32);
        crate::space::Space::<H>::query_multi_raw(
            unsafe { prz_ptr.cast_mut().as_mut().unwrap() },
            &[ExprEnv::new(
                0,
                Expr {
                    ptr: v.as_ptr().cast_mut(),
                },
            )],
            |refs_bindings, loc| {
                for b in prz.child_mask().and(&ByteMask(crate::space::SIZES)).iter() {
                    let Tag::SymbolSize(size) = byte_item(b) else {
                        unreachable!()
                    };
                    // if size != 16 { trace!(target: "sink", "hash guard not 16 bytes {size}"); continue }
                    prz.descend_to_byte(b);
                    debug_assert!(prz.path_exists());
                    if !prz.descend_first_k_path(size as _) {
                        unreachable!()
                    }
                    loop {
                        let clen = prz.origin_path().len();

                        let hash = prz.fork_read_zipper().hash();

                        let cnt_str = hash.to_be_bytes();
                        trace!(target: "sink", "'{}' and under {}", serialize(prz.origin_path()), hash);
                        assert_eq!(prz.origin_path().len(), clen);

                        let fixed_number =
                            &prz.origin_path()[prz.origin_path().len() - (size as usize)..];
                        if fixed_number == &cnt_str[..] {
                            let fixed =
                                &prz.origin_path()[..prz.origin_path().len() - (1 + size as usize)];
                            trace!(target: "sink", "fixed payload {}", serialize(fixed));
                            wz.move_to_path(fixed);
                            wz.set_val(H::default());
                            changed |= true;
                        }

                        if !prz.to_next_k_path(size as _) {
                            break;
                        }
                    }
                    if !prz.ascend_byte() {
                        unreachable!()
                    }
                }

                if prz.descend_to_existing_byte(item_byte(Tag::NewVar)) {
                    let ignored = &prz.path()[..prz.path().len() - 1];
                    trace!(target: "sink", "ignored guard {}", serialize(ignored));
                    wz.move_to_path(ignored);
                    wz.set_val(H::default());
                    changed |= true;
                    prz.ascend_byte();
                }
                if prz.descend_first_byte() {
                    if let Tag::VarRef(k) = byte_item(prz.path()[prz.path().len() - 1]) {
                        let hash = prz.fork_read_zipper().hash();
                        let cnt_str = hash.to_be_bytes();

                        let mut cntv = vec![item_byte(Tag::SymbolSize(cnt_str.len() as _))];
                        cntv.extend_from_slice(&cnt_str[..]);
                        let varref = &prz.path()[..prz.path().len() - 1];
                        let ie = Expr {
                            ptr: (&varref[0] as *const u8).cast_mut(),
                        };
                        let mut oz = ExprZipper::new(Expr {
                            ptr: buffer.as_mut_ptr(),
                        });
                        trace!(target: "sink", "hash ref guard '{}' var {:?} with '{}'", serialize(varref), k, serialize(&cntv[..]));
                        let os = ie.substitute_one_de_bruijn(
                            k,
                            Expr {
                                ptr: cntv.as_mut_ptr(),
                            },
                            &mut oz,
                        );
                        unsafe { buffer.set_len(oz.loc) }
                        trace!(target: "sink", "hash ref guard subs '{:?}'", serialize(&buffer[..oz.loc]));
                        wz.move_to_path(&buffer[wz.root_prefix_path().len()..oz.loc]);
                        wz.set_val(H::default());
                        changed |= true
                    }
                    prz.ascend_byte();
                }
                true
            },
        );
        changed
    }
}

pub struct AndSink {
    e: Expr,
    unique: PathMap<()>,
}
impl<H> Sink<H> for AndSink
where
    H: AtomHeader + Default,
{
    fn new(e: Expr) -> Self {
        Self {
            e,
            unique: PathMap::new(),
        }
    }
    fn request(&self) -> impl Iterator<Item = WriteResourceRequest> {
        let p = &unsafe {
            self.e
                .prefix()
                .unwrap_or_else(|x| {
                    let s = self.e.span();
                    slice_from_raw_parts(self.e.ptr, s.len() - 1)
                })
                .as_ref()
                .unwrap()
        }[5..];
        trace!(target: "sink", "and requesting {}", serialize(p));
        std::iter::once(WriteResourceRequest::BTM(p))
    }
    fn sink<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
        path: &[u8],
    ) where
        'a: 'w,
        'k: 'w,
    {
        let WriteResource::BTM(wz) = it.next().unwrap() else {
            unreachable!()
        };
        let mpath = &path[5 + wz.root_prefix_path().len()..];
        let ctx = unsafe {
            Expr {
                ptr: mpath.as_ptr().cast_mut(),
            }
        };
        trace!(target: "sink", "and at '{}' sinking raw '{}'", serialize(wz.root_prefix_path()), serialize(path));
        trace!(target: "sink", "and registering in ctx {:?}", serialize(mpath));
        self.unique.insert(mpath, ());
    }
    fn finalize<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
    ) -> bool
    where
        'a: 'w,
        'k: 'w,
    {
        let WriteResource::BTM(wz) = it.next().unwrap() else {
            unreachable!()
        };
        wz.reset();
        trace!(target: "sink", "and finalizing by reducing {} at '{}'", self.unique.val_count(), serialize(wz.origin_path()));

        let mut _to_swap = PathMap::new();
        std::mem::swap(&mut self.unique, &mut _to_swap);
        let mut rooted_input = PathMap::new();
        rooted_input
            .write_zipper_at_path(wz.root_prefix_path())
            .graft_map(_to_swap);

        static v: &'static [u8] = &[item_byte(Tag::NewVar)];
        let mut prz = OneFactor::new(rooted_input.into_read_zipper(&[]));
        let prz_ptr = (&prz) as *const OneFactor<_>;
        let mut changed = false;
        let mut buffer: Vec<u8> = Vec::with_capacity(1 << 32);
        crate::space::Space::<H>::query_multi_raw(
            unsafe { prz_ptr.cast_mut().as_mut().unwrap() },
            &[ExprEnv::new(
                0,
                Expr {
                    ptr: v.as_ptr().cast_mut(),
                },
            )],
            |refs_bindings, loc| {
                for b in prz.child_mask().and(&ByteMask(crate::space::SIZES)).iter() {
                    let Tag::SymbolSize(size) = byte_item(b) else {
                        unreachable!()
                    };
                    prz.descend_to_byte(b);
                    debug_assert!(prz.path_exists());
                    if !prz.descend_first_k_path(size as _) {
                        unreachable!()
                    }
                    loop {
                        let mut total = !0u8;
                        let clen = prz.origin_path().len();

                        let mut rz = prz.fork_read_zipper();
                        while rz.to_next_val() {
                            let p = rz.origin_path();
                            trace!(target: "sink", "path number {:?}", serialize(&p[clen..]));
                            total &= p[clen + 1];
                        }
                        let cnt_str = [total];
                        trace!(target: "sink", "'{}' and under {}", serialize(prz.origin_path()), total);
                        assert_eq!(prz.origin_path().len(), clen);

                        let fixed_number =
                            &prz.origin_path()[prz.origin_path().len() - (size as usize)..];
                        if fixed_number == &cnt_str[..] {
                            let fixed =
                                &prz.origin_path()[..prz.origin_path().len() - (1 + size as usize)];
                            trace!(target: "sink", "fixed payload {}", serialize(fixed));
                            wz.move_to_path(fixed);
                            wz.set_val(H::default());
                            changed |= true;
                        }

                        if !prz.to_next_k_path(size as _) {
                            break;
                        }
                    }
                    if !prz.ascend_byte() {
                        unreachable!()
                    }
                }

                if prz.descend_to_existing_byte(item_byte(Tag::NewVar)) {
                    let ignored = &prz.path()[..prz.path().len() - 1];
                    trace!(target: "sink", "ignored guard {}", serialize(ignored));
                    wz.move_to_path(ignored);
                    wz.set_val(H::default());
                    changed |= true;
                    prz.ascend_byte();
                }
                if prz.descend_first_byte() {
                    if let Tag::VarRef(k) = byte_item(prz.path()[prz.path().len() - 1]) {
                        let mut total = !0u8;
                        let clen = prz.path().len();
                        let mut rz = prz.fork_read_zipper();
                        while rz.to_next_val() {
                            let p = rz.origin_path();
                            trace!(target: "sink", "and path {:?}", serialize(p));
                            trace!(target: "sink", "and path {:?}", serialize(&p[clen+1..]));
                            total &= p[clen + 1];
                        }
                        let cnt_str = [total];

                        let mut cntv = vec![item_byte(Tag::SymbolSize(cnt_str.len() as _))];
                        cntv.extend_from_slice(&cnt_str[..]);
                        let varref = &prz.path()[..prz.path().len() - 1];
                        let ie = Expr {
                            ptr: (&varref[0] as *const u8).cast_mut(),
                        };
                        let mut oz = ExprZipper::new(Expr {
                            ptr: buffer.as_mut_ptr(),
                        });
                        trace!(target: "sink", "and ref guard '{}' var {:?} with '{}'", serialize(varref), k, serialize(&cntv[..]));
                        let os = ie.substitute_one_de_bruijn(
                            k,
                            Expr {
                                ptr: cntv.as_mut_ptr(),
                            },
                            &mut oz,
                        );
                        unsafe { buffer.set_len(oz.loc) }
                        trace!(target: "sink", "and ref guard subs '{:?}'", serialize(&buffer[..oz.loc]));
                        wz.move_to_path(&buffer[wz.root_prefix_path().len()..oz.loc]);
                        wz.set_val(H::default());
                        changed |= true
                    }
                    prz.ascend_byte();
                }
                true
            },
        );
        changed
    }
}

pub struct SumSink {
    e: Expr,
    unique: PathMap<()>,
}
impl<H> Sink<H> for SumSink
where
    H: AtomHeader + Default,
{
    fn new(e: Expr) -> Self {
        SumSink {
            e,
            unique: PathMap::new(),
        }
    }
    fn request(&self) -> impl Iterator<Item = WriteResourceRequest> {
        let p = &unsafe {
            self.e
                .prefix()
                .unwrap_or_else(|x| {
                    let s = self.e.span();
                    slice_from_raw_parts(self.e.ptr, s.len() - 1)
                })
                .as_ref()
                .unwrap()
        }[5..];
        trace!(target: "sink", "sum requesting {}", serialize(p));
        std::iter::once(WriteResourceRequest::BTM(p))
    }
    fn sink<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
        path: &[u8],
    ) where
        'a: 'w,
        'k: 'w,
    {
        let WriteResource::BTM(wz) = it.next().unwrap() else {
            unreachable!()
        };
        let mpath = &path[5 + wz.root_prefix_path().len()..];
        let ctx = unsafe {
            Expr {
                ptr: mpath.as_ptr().cast_mut(),
            }
        };
        trace!(target: "sink", "sum at '{}' sinking raw '{}'", serialize(wz.root_prefix_path()), serialize(path));
        trace!(target: "sink", "sum registering in ctx {:?}", serialize(mpath));
        self.unique.insert(mpath, ());
    }
    fn finalize<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
    ) -> bool
    where
        'a: 'w,
        'k: 'w,
    {
        let WriteResource::BTM(wz) = it.next().unwrap() else {
            unreachable!()
        };
        wz.reset();
        trace!(target: "sink", "sum finalizing by reducing {} at '{}'", self.unique.val_count(), serialize(wz.origin_path()));

        let mut _to_swap = PathMap::new();
        std::mem::swap(&mut self.unique, &mut _to_swap);
        let mut rooted_input = PathMap::new();
        rooted_input
            .write_zipper_at_path(wz.root_prefix_path())
            .graft_map(_to_swap);

        static v: &'static [u8] = &[item_byte(Tag::NewVar)];
        let mut prz = OneFactor::new(rooted_input.into_read_zipper(&[]));
        let prz_ptr = (&prz) as *const OneFactor<_>;
        let mut changed = false;
        let mut buffer: Vec<u8> = Vec::with_capacity(1 << 32);
        crate::space::Space::<H>::query_multi_raw(
            unsafe { prz_ptr.cast_mut().as_mut().unwrap() },
            &[ExprEnv::new(
                0,
                Expr {
                    ptr: v.as_ptr().cast_mut(),
                },
            )],
            |refs_bindings, loc| {
                for b in prz.child_mask().and(&ByteMask(crate::space::SIZES)).iter() {
                    let Tag::SymbolSize(size) = byte_item(b) else {
                        unreachable!()
                    };
                    prz.descend_to_byte(b);
                    debug_assert!(prz.path_exists());
                    if !prz.descend_first_k_path(size as _) {
                        unreachable!()
                    }
                    loop {
                        let mut total = 0u32;
                        let clen = prz.origin_path().len();

                        let mut rz = prz.fork_read_zipper();
                        while rz.to_next_val() {
                            let p = rz.origin_path();
                            trace!(target: "sink", "path number {:?}", serialize(&p[clen..]));
                            total +=
                                u32::from_str_radix(str::from_utf8(&p[clen + 1..]).unwrap(), 10)
                                    .unwrap();
                        }
                        let cnt_str = total.to_string();
                        trace!(target: "sink", "'{}' and under {}", serialize(prz.origin_path()), total);
                        assert_eq!(prz.origin_path().len(), clen);

                        let fixed_number =
                            &prz.origin_path()[prz.origin_path().len() - (size as usize)..];
                        if fixed_number == cnt_str.as_bytes() {
                            let fixed =
                                &prz.origin_path()[..prz.origin_path().len() - (1 + size as usize)];
                            trace!(target: "sink", "fixed payload {}", serialize(fixed));
                            wz.move_to_path(fixed);
                            wz.set_val(H::default());
                            changed |= true;
                        }

                        if !prz.to_next_k_path(size as _) {
                            break;
                        }
                    }
                    if !prz.ascend_byte() {
                        unreachable!()
                    }
                }

                if prz.descend_to_existing_byte(item_byte(Tag::NewVar)) {
                    let ignored = &prz.path()[..prz.path().len() - 1];
                    trace!(target: "sink", "ignored guard {}", serialize(ignored));
                    wz.move_to_path(ignored);
                    wz.set_val(H::default());
                    changed |= true;
                    prz.ascend_byte();
                }
                if prz.descend_first_byte() {
                    if let Tag::VarRef(k) = byte_item(prz.path()[prz.path().len() - 1]) {
                        let mut total = 0u32;
                        let clen = prz.path().len();
                        let mut rz = prz.fork_read_zipper();
                        while rz.to_next_val() {
                            let p = rz.origin_path();
                            trace!(target: "sink", "path {:?}", serialize(p));
                            trace!(target: "sink", "path {:?}", serialize(&p[clen+1..]));
                            total +=
                                u32::from_str_radix(str::from_utf8(&p[clen + 1..]).unwrap(), 10)
                                    .unwrap();
                        }
                        let cnt_str = total.to_string();

                        let mut cntv = vec![item_byte(Tag::SymbolSize(cnt_str.len() as _))];
                        cntv.extend_from_slice(cnt_str.as_bytes());
                        let varref = &prz.path()[..prz.path().len() - 1];
                        let ie = Expr {
                            ptr: (&varref[0] as *const u8).cast_mut(),
                        };
                        let mut oz = ExprZipper::new(Expr {
                            ptr: buffer.as_mut_ptr(),
                        });
                        trace!(target: "sink", "ref guard '{}' var {:?} with '{}'", serialize(varref), k, serialize(&cntv[..]));
                        let os = ie.substitute_one_de_bruijn(
                            k,
                            Expr {
                                ptr: cntv.as_mut_ptr(),
                            },
                            &mut oz,
                        );
                        unsafe { buffer.set_len(oz.loc) }
                        trace!(target: "sink", "ref guard subs '{:?}'", serialize(&buffer[..oz.loc]));
                        wz.move_to_path(&buffer[wz.root_prefix_path().len()..oz.loc]);
                        wz.set_val(H::default());
                        changed |= true
                    }
                    prz.ascend_byte();
                }
                true
            },
        );
        changed
    }
}

// (pure (result $x) $x (f32_from_string 0.2))
#[cfg(feature = "grounding")]
pub struct PureSink<H: AtomHeader + Default> {
    e: Expr,
    unique: PathMap<H>,
    scope: EvalScope,
}
impl<H> Sink<H> for PureSink<H>
where
    H: AtomHeader + Default,
{
    fn new(e: Expr) -> Self {
        let mut scope = EvalScope::new();
        pure::register(&mut scope);
        PureSink {
            e,
            unique: PathMap::<H>::new(),
            scope,
        }
    }
    fn request(&self) -> impl Iterator<Item = WriteResourceRequest> {
        let p = &unsafe {
            self.e
                .prefix()
                .unwrap_or_else(|x| {
                    let s = self.e.span();
                    slice_from_raw_parts(self.e.ptr, s.len() - 1)
                })
                .as_ref()
                .unwrap()
        }[6..];
        trace!(target: "sink", "count requesting {}", serialize(p));
        std::iter::once(WriteResourceRequest::BTM(p))
    }
    fn sink<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
        path: &[u8],
    ) where
        'a: 'w,
        'k: 'w,
    {
        let WriteResource::BTM(wz) = it.next().unwrap() else {
            unreachable!()
        };
        let mpath = &path[6 + wz.root_prefix_path().len()..];
        let ctx = unsafe {
            Expr {
                ptr: mpath.as_ptr().cast_mut(),
            }
        };
        trace!(target: "sink", "pure at '{}' sinking raw '{}'", serialize(wz.root_prefix_path()), serialize(path));
        trace!(target: "sink", "pure registering in ctx {:?}", serialize(mpath));
        self.unique.insert(mpath, H::default());
    }
    fn finalize<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
    ) -> bool
    where
        'a: 'w,
        'k: 'w,
    {
        let WriteResource::BTM(wz) = it.next().unwrap() else {
            unreachable!()
        };
        wz.reset();
        trace!(target: "sink", "pure finalizing by reducing {} at '{}'", self.unique.val_count(), serialize(wz.origin_path()));

        let mut _to_swap = PathMap::new();
        std::mem::swap(&mut self.unique, &mut _to_swap);
        let mut rooted_input = PathMap::new();
        rooted_input
            .write_zipper_at_path(wz.root_prefix_path())
            .graft_map(_to_swap);

        static v: &'static [u8] = &[item_byte(Tag::NewVar)];
        let mut prz = OneFactor::new(rooted_input.into_read_zipper(&[]));
        let prz_ptr = (&prz) as *const OneFactor<_>;
        let mut changed = false;
        let mut buffer: Vec<u8> = Vec::with_capacity(1 << 32);
        crate::space::Space::<H>::query_multi_raw(
            unsafe { prz_ptr.cast_mut().as_mut().unwrap() },
            &[ExprEnv::new(
                0,
                Expr {
                    ptr: v.as_ptr().cast_mut(),
                },
            )],
            |refs_bindings, loc| {
                for b in prz.child_mask().and(&ByteMask(crate::space::SIZES)).iter() {
                    let Tag::SymbolSize(size) = byte_item(b) else {
                        unreachable!()
                    };
                    prz.descend_to_byte(b);
                    debug_assert!(prz.path_exists());
                    if !prz.descend_first_k_path(size as _) {
                        unreachable!()
                    }
                    loop {
                        let clen = prz.origin_path().len();

                        let mut rz = prz.fork_read_zipper();
                        'vals: while rz.to_next_val() {
                            let p = rz.origin_path();
                            trace!(target: "sink", "path number {:?}", serialize(&p[clen..]));
                            let mut res = match self.scope.eval(ExprSource::new(&p[clen])) {
                                Ok(res) => res,
                                Err(er) => {
                                    trace!(target: "pure", "err {}", er);
                                    continue 'vals;
                                }
                            };
                            trace!(target: "sink", "symbol result {:?}", serialize(&res[..]));
                            wz.move_to_path(&res[wz.root_prefix_path().len()..]);
                            wz.set_val(H::default());
                            changed |= true;
                            self.scope.return_alloc(res);
                        }

                        if !prz.to_next_k_path(size as _) {
                            break;
                        }
                    }
                    if !prz.ascend_byte() {
                        unreachable!()
                    }
                }

                for b in prz
                    .child_mask()
                    .and(&ByteMask(crate::space::ARITIES))
                    .iter()
                {
                    prz.descend_to_byte(b);
                    if prz.path_exists() {
                        let clen2 = prz.origin_path().len();
                        let mut rz2 = prz.fork_read_zipper();
                        while rz2.to_next_val() {
                            let p2 = rz2.origin_path();
                            trace!(target: "sink", "arity val {:?}", serialize(&p2[clen2..]));
                            let mut res = match self.scope.eval(ExprSource::new(&p2[clen2])) {
                                Ok(res) => res,
                                Err(er) => {
                                    trace!(target: "pure", "err {}", er);
                                    continue;
                                }
                            };
                            trace!(target: "sink", "arity result {:?}", serialize(&res[..]));
                            wz.move_to_path(&res[wz.root_prefix_path().len()..]);
                            wz.set_val(H::default());
                            changed |= true;
                            self.scope.return_alloc(res);
                        }
                    }
                    if !prz.ascend_byte() {
                        unreachable!()
                    }
                }

                if prz.descend_to_existing_byte(item_byte(Tag::NewVar)) {
                    let ignored = &prz.path()[..prz.path().len() - 1];
                    trace!(target: "sink", "ignored guard {}", serialize(ignored));
                    wz.move_to_path(ignored);
                    wz.set_val(H::default());
                    changed |= true;
                    prz.ascend_byte();
                }
                if prz.descend_first_byte() {
                    if let Tag::VarRef(k) = byte_item(prz.path()[prz.path().len() - 1]) {
                        let clen = prz.path().len();
                        let mut rz = prz.fork_read_zipper();
                        'vals: while rz.to_next_val() {
                            let p = rz.origin_path();
                            trace!(target: "sink", "path {:?}", serialize(p));
                            trace!(target: "sink", "path {:?}", serialize(&p[clen..]));

                            let mut res = match self.scope.eval(ExprSource::new(&p[clen])) {
                                Ok(res) => res,
                                Err(er) => {
                                    trace!(target: "pure", "err {}", er);
                                    continue 'vals;
                                }
                            };

                            trace!(target: "sink", "result {:?}", serialize(&res[..]));

                            let varref = &prz.path()[..prz.path().len() - 1];
                            let ie = Expr {
                                ptr: (&varref[0] as *const u8).cast_mut(),
                            };
                            let mut oz = ExprZipper::new(Expr {
                                ptr: buffer.as_mut_ptr(),
                            });
                            trace!(target: "sink", "ref guard '{}' var {:?} with '{}'", serialize(varref), k, serialize(&res[..]));
                            let os = ie.substitute_one_de_bruijn(
                                k,
                                Expr {
                                    ptr: res.as_mut_ptr(),
                                },
                                &mut oz,
                            );
                            unsafe { buffer.set_len(oz.loc) }
                            trace!(target: "sink", "ref guard subs '{:?}'", serialize(&buffer[..oz.loc]));
                            wz.move_to_path(&buffer[wz.root_prefix_path().len()..oz.loc]);
                            wz.set_val(H::default());
                            changed |= true;
                            self.scope.return_alloc(res);
                        }
                    }
                    prz.ascend_byte();
                }
                true
            },
        );
        changed
    }
}

// (z3 <instance> <declaration or assertion>)
#[cfg(feature = "z3")]
pub struct Z3Sink {
    e: Expr,
    buffer: Vec<u8>,
}
#[cfg(feature = "z3")]
impl<H> Sink<H> for Z3Sink {
    fn new(e: Expr) -> Self {
        Z3Sink { e, buffer: vec![] }
    }
    fn request(&self) -> impl Iterator<Item = WriteResourceRequest> {
        destruct!(self.e, ("z3" {instance: &str} {decl: Expr}), {
            trace!(target: "sinks", "z3 requesting instance {instance}");
            return std::iter::once(WriteResourceRequest::Z3(instance));
        }, _err => { unreachable!() });
    }
    fn sink<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k>>>(
        &mut self,
        mut it: It,
        path: &[u8],
    ) where
        'a: 'w,
        'k: 'w,
    {
        trace!(target: "sink", "z3 at raw '{}'", serialize(path));
        let e = Expr {
            ptr: path.as_ptr().cast_mut(),
        };
        e.serialize(&mut self.buffer, |e| std::str::from_utf8(e).unwrap());
        self.buffer.push(b'\n');
    }
    fn finalize<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k>>>(
        &mut self,
        mut it: It,
    ) -> bool
    where
        'a: 'w,
        'k: 'w,
    {
        trace!(target: "sink", "z3 writing buffer {}", std::str::from_utf8(&self.buffer[..]).unwrap());
        let WriteResource::Z3(ref mut p) = it.next().unwrap() else {
            unreachable!()
        };
        let mut stdin = p.stdin.as_mut().unwrap();
        stdin.write(&self.buffer[..]).unwrap();
        true
    }
}

pub struct WsSink {
    e: Expr,
    skip: usize,
    delta: u64,
}
impl<H> Sink<H> for WsSink
where
    H: AtomHeader + Default,
{
    fn new(e: Expr) -> Self {
        let mut ez = ExprZipper::new(e);
        ez.next();
        ez.next();
        let max_s = ez
            .item()
            .err()
            .expect("cnt can not be an expression or variable");
        let delta: u64 = str::from_utf8(max_s)
            .expect("string encoded numbers for now")
            .parse()
            .expect("a number");
        WsSink {
            e,
            skip: 1 + 1 + 2 + 1 + max_s.len(),
            delta,
        }
    }

    fn request(&self) -> impl Iterator<Item = WriteResourceRequest> {
        let p = &unsafe {
            self.e
                .prefix()
                .unwrap_or_else(|x| {
                    let s = self.e.span();
                    slice_from_raw_parts(self.e.ptr, s.len() - 1)
                })
                .as_ref()
                .unwrap()
        }[4..];
        trace!(target: "sink", "ws requesting {}", serialize(p));
        // if let Some(_) = GLOBAL_WS_SWEEP.get() {
        let mut guard = GLOBAL_WS_SWEEP.lock().unwrap();
        if guard.is_some() {
            trace!(target: "sink", "WSP available");
        } else {
            let wsp = init_weight();
            // GLOBAL_WS_SWEEP.set(std::sync::Arc::new(wsp));
            *guard = Some(wsp);
            trace!(target: "sink", "create wsp");
        }
        std::iter::once(WriteResourceRequest::BTM(p))
    }

    fn sink<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
        path: &[u8],
    ) where
        'a: 'w,
        'k: 'w,
    {
        let current_thread = std::thread::current().id();
        trace!(target: "sink", "ws at '{}' sinking raw '{}'", serialize(path), serialize(path));
        // if let Some(wsweep) = GLOBAL_WS_SWEEP.get() {
        let mut guard = GLOBAL_WS_SWEEP.lock().unwrap();
        if let Some(wsweep) = guard.as_mut() {
            let WriteResource::BTM(wz) = it.next().unwrap() else {
                unreachable!()
            };
            let mpath = &path[self.skip..];
            let wmap = &wsweep.map;
            trace!(target: "sink", "ws at '{}' sinking raw '{}'", serialize(wz.root_prefix_path()), serialize(path));
            trace!(target: "sink", "ws sinking '{}'", serialize(mpath));
            let curr = wmap.get_val(mpath).unwrap_or(U64AtomHeader::default()).0;
            let _ = &wmap.set_weighted_val(mpath, U64AtomHeader((curr as u64 + self.delta) as i32));

            trace!(target: "sink", "ws finalizing {:?}", wmap.get_val(mpath).unwrap_or(U64AtomHeader::default()).0);
        } else {
            trace!(target: "sink", "WSwp missing!");
        };
    }

    fn finalize<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        mut it: It,
    ) -> bool
    where
        'a: 'w,
        'k: 'w,
    {
        true
    }
}

pub enum ASink<H: AtomHeader + Default> {
    AddSink(AddSink),
    // RemoveSink(RemoveSink),
    // HeadSink(HeadSink),
    CountSink(CountSink),
    HashSink(HashSink),
    SumSink(SumSink),
    AndSink(AndSink),
    ACTSink(ACTSink),
    #[cfg(feature = "wasm")]
    WASMSink(WASMSink),
    #[cfg(feature = "grounding")]
    PureSink(PureSink<H>),
    #[cfg(feature = "z3")]
    Z3Sink(Z3Sink),
    AUSink(AUSink),
    USink(USink),
    CompatSink(CompatSink),
    WsSink(WsSink),
}

impl<H: AtomHeader + Default> ASink<H> {
    pub fn compat(e: Expr) -> Self {
        ASink::CompatSink(<CompatSink as Sink<H>>::new(e))
    }
}

impl<H: AtomHeader + Default> Sink<H> for ASink<H> {
    fn new(e: Expr) -> Self {
        // if unsafe { *e.ptr == item_byte(Tag::Arity(2)) && *e.ptr.offset(1) == item_byte(Tag::SymbolSize(1)) && *e.ptr.offset(2) == b'-' } {
        //     ASink::RemoveSink(RemoveSink::new(e))
        // } else
        if unsafe {
            *e.ptr == item_byte(Tag::Arity(2))
                && *e.ptr.offset(1) == item_byte(Tag::SymbolSize(1))
                && *e.ptr.offset(2) == b'+'
        } {
            ASink::AddSink(<AddSink as Sink<H>>::new(e))
        } else if unsafe {
            *e.ptr == item_byte(Tag::Arity(2))
                && *e.ptr.offset(1) == item_byte(Tag::SymbolSize(1))
                && *e.ptr.offset(2) == b'U'
        } {
            ASink::USink(<USink as Sink<H>>::new(e))
        } else if unsafe {
            *e.ptr == item_byte(Tag::Arity(2))
                && *e.ptr.offset(1) == item_byte(Tag::SymbolSize(2))
                && *e.ptr.offset(2) == b'A'
                && *e.ptr.offset(3) == b'U'
        } {
            ASink::AUSink(<AUSink as Sink<H>>::new(e))
        // } else if unsafe { *e.ptr == item_byte(Tag::Arity(3)) && *e.ptr.offset(1) == item_byte(Tag::SymbolSize(4)) &&
        //     *e.ptr.offset(2) == b'h' && *e.ptr.offset(3) == b'e' && *e.ptr.offset(4) == b'a' && *e.ptr.offset(5) == b'd' } {
        //     ASink::HeadSink(HeadSink::new(e))
        } else if unsafe {
            *e.ptr == item_byte(Tag::Arity(4))
                && *e.ptr.offset(1) == item_byte(Tag::SymbolSize(5))
                && *e.ptr.offset(2) == b'c'
                && *e.ptr.offset(3) == b'o'
                && *e.ptr.offset(4) == b'u'
                && *e.ptr.offset(5) == b'n'
                && *e.ptr.offset(6) == b't'
        } {
            ASink::CountSink(<CountSink as Sink<H>>::new(e))
        } else if unsafe {
            *e.ptr == item_byte(Tag::Arity(4))
                && *e.ptr.offset(1) == item_byte(Tag::SymbolSize(4))
                && *e.ptr.offset(2) == b'h'
                && *e.ptr.offset(3) == b'a'
                && *e.ptr.offset(4) == b's'
                && *e.ptr.offset(5) == b'h'
        } {
            ASink::HashSink(<HashSink as Sink<H>>::new(e))
        } else if unsafe {
            *e.ptr == item_byte(Tag::Arity(4))
                && *e.ptr.offset(1) == item_byte(Tag::SymbolSize(3))
                && *e.ptr.offset(2) == b's'
                && *e.ptr.offset(3) == b'u'
                && *e.ptr.offset(4) == b'm'
        } {
            return ASink::SumSink(<SumSink as Sink<H>>::new(e));
        } else if unsafe {
            *e.ptr == item_byte(Tag::Arity(4))
                && *e.ptr.offset(1) == item_byte(Tag::SymbolSize(3))
                && *e.ptr.offset(2) == b'a'
                && *e.ptr.offset(3) == b'n'
                && *e.ptr.offset(4) == b'd'
        } {
            return ASink::AndSink(<AndSink as Sink<H>>::new(e));
        } else if unsafe {
            *e.ptr == item_byte(Tag::Arity(3))
                && *e.ptr.offset(1) == item_byte(Tag::SymbolSize(3))
                && *e.ptr.offset(2) == b'A'
                && *e.ptr.offset(3) == b'C'
                && *e.ptr.offset(4) == b'T'
        } {
            return ASink::ACTSink(<ACTSink as Sink<H>>::new(e));
        } else if unsafe {
            *e.ptr == item_byte(Tag::Arity(3))
                && *e.ptr.offset(1) == item_byte(Tag::SymbolSize(4))
                && *e.ptr.offset(2) == b'w'
                && *e.ptr.offset(3) == b'a'
                && *e.ptr.offset(4) == b's'
                && *e.ptr.offset(5) == b'm'
        } {
            #[cfg(feature = "wasm")]
            return ASink::WASMSink(WASMSink::new(e));
            #[cfg(not(feature = "wasm"))]
            panic!(
                "MORK was not built with the wasm feature, yet trying to call {:?}",
                e
            );
        } else if unsafe {
            *e.ptr == item_byte(Tag::Arity(4))
                && *e.ptr.offset(1) == item_byte(Tag::SymbolSize(4))
                && *e.ptr.offset(2) == b'p'
                && *e.ptr.offset(3) == b'u'
                && *e.ptr.offset(4) == b'r'
                && *e.ptr.offset(5) == b'e'
        } {
            #[cfg(feature = "grounding")]
            return ASink::PureSink(PureSink::new(e));
            #[cfg(not(feature = "grounding"))]
            panic!(
                "MORK was not built with the grounding feature, yet trying to call {:?}",
                e
            );
        } else if unsafe {
            *e.ptr == item_byte(Tag::Arity(3))
                && *e.ptr.offset(1) == item_byte(Tag::SymbolSize(2))
                && *e.ptr.offset(2) == b'z'
                && *e.ptr.offset(3) == b'3'
        } {
            #[cfg(feature = "z3")]
            return ASink::Z3Sink(Z3Sink::new(e));
            #[cfg(not(feature = "z3"))]
            panic!(
                "MORK was not built with the z3 feature, yet trying to call {:?}",
                e
            );
        } else if unsafe {
            *e.ptr == item_byte(Tag::Arity(3))
                && *e.ptr.offset(1) == item_byte(Tag::SymbolSize(2))
                && *e.ptr.offset(2) == b'w'
                && *e.ptr.offset(3) == b's'
        } {
            ASink::WsSink(<WsSink as Sink<H>>::new(e))
        } else {
            panic!("unrecognized sink")
        }
    }

    fn request(&self) -> impl Iterator<Item = WriteResourceRequest> {
        gen move {
            match self {
                ASink::AddSink(s) => {
                    for i in Sink::<H>::request(s).into_iter() {
                        yield i
                    }
                }
                ASink::USink(s) => {
                    for i in Sink::<H>::request(s).into_iter() {
                        yield i
                    }
                }
                ASink::AUSink(s) => {
                    for i in Sink::<H>::request(s).into_iter() {
                        yield i
                    }
                }
                // ASink::RemoveSink(s) => { for i in s.request().into_iter() { yield i } }
                // ASink::HeadSink(s) => { for i in s.request().into_iter() { yield i } }
                ASink::CountSink(s) => {
                    for i in Sink::<H>::request(s).into_iter() {
                        yield i
                    }
                }
                ASink::HashSink(s) => {
                    for i in Sink::<H>::request(s).into_iter() {
                        yield i
                    }
                }
                ASink::SumSink(s) => {
                    for i in Sink::<H>::request(s).into_iter() {
                        yield i
                    }
                }
                ASink::AndSink(s) => {
                    for i in Sink::<H>::request(s).into_iter() {
                        yield i
                    }
                }
                ASink::ACTSink(s) => {
                    for i in Sink::<H>::request(s).into_iter() {
                        yield i
                    }
                }
                #[cfg(feature = "wasm")]
                ASink::WASMSink(s) => {
                    for i in s.request().into_iter() {
                        yield i
                    }
                }
                #[cfg(feature = "grounding")]
                ASink::PureSink(s) => {
                    for i in s.request().into_iter() {
                        yield i
                    }
                }
                #[cfg(feature = "z3")]
                ASink::Z3Sink(s) => {
                    for i in s.request().into_iter() {
                        yield i
                    }
                }
                ASink::CompatSink(s) => {
                    for i in Sink::<H>::request(s).into_iter() {
                        yield i
                    }
                }

                ASink::WsSink(s) => {
                    for i in Sink::<H>::request(s).into_iter() {
                        yield i
                    }
                }
            }
        }
    }
    fn sink<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        it: It,
        path: &[u8],
    ) where
        'a: 'w,
        'k: 'w,
    {
        match self {
            ASink::AddSink(s) => s.sink(it, path),
            ASink::USink(s) => s.sink(it, path),
            ASink::AUSink(s) => s.sink(it, path),
            // ASink::RemoveSink(s) => { s.sink(it, path) }
            // ASink::HeadSink(s) => { s.sink(it, path) }
            ASink::CountSink(s) => s.sink(it, path),
            ASink::HashSink(s) => s.sink(it, path),
            ASink::SumSink(s) => s.sink(it, path),
            ASink::AndSink(s) => s.sink(it, path),
            ASink::ACTSink(s) => s.sink(it, path),
            #[cfg(feature = "wasm")]
            ASink::WASMSink(s) => s.sink(it, path),
            #[cfg(feature = "grounding")]
            ASink::PureSink(s) => s.sink(it, path),
            #[cfg(feature = "z3")]
            ASink::Z3Sink(s) => s.sink(it, path),
            ASink::CompatSink(s) => s.sink(it, path),
            ASink::WsSink(s) => s.sink(it, path),
        }
    }

    fn finalize<'w, 'a, 'k, It: Iterator<Item = WriteResource<'w, 'a, 'k, H>>>(
        &mut self,
        it: It,
    ) -> bool
    where
        'a: 'w,
        'k: 'w,
    {
        match self {
            ASink::AddSink(s) => s.finalize(it),
            ASink::USink(s) => s.finalize(it),
            ASink::AUSink(s) => s.finalize(it),
            // ASink::RemoveSink(s) => { s.finalize(it) }
            // ASink::HeadSink(s) => { s.finalize(it) }
            ASink::CountSink(s) => s.finalize(it),
            ASink::HashSink(s) => s.finalize(it),
            ASink::SumSink(s) => s.finalize(it),
            ASink::AndSink(s) => s.finalize(it),
            ASink::ACTSink(s) => s.finalize(it),
            #[cfg(feature = "wasm")]
            ASink::WASMSink(s) => s.finalize(it),
            #[cfg(feature = "grounding")]
            ASink::PureSink(s) => s.finalize(it),
            #[cfg(feature = "z3")]
            ASink::Z3Sink(s) => s.finalize(it),
            ASink::CompatSink(s) => s.finalize(it),
            ASink::WsSink(s) => s.finalize(it),
        }
    }
}
