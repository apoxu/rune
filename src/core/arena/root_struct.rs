use std::fmt::Debug;
use std::ops::{Deref, DerefMut, Index, IndexMut};
use std::ptr::{addr_of, addr_of_mut};
use std::slice::SliceIndex;

use super::super::{
    cons::Cons,
    env::{Environment, Symbol},
    object::{GcObj, RawObj},
};
use super::{Arena, RootSet, Trace};
use crate::core::object::{Gc, WithLifetime};
use crate::hashmap::HashMap;

use qcell::{LCell, LCellOwner};

pub(crate) trait IntoRoot<T> {
    unsafe fn into_root(self) -> T;
}

impl<'ob> IntoRoot<RootObj> for GcObj<'ob> {
    unsafe fn into_root(self) -> RootObj {
        RootObj::new(self)
    }
}

impl IntoRoot<RootCons> for &Cons {
    unsafe fn into_root(self) -> RootCons {
        RootCons::new(self)
    }
}

impl<'ob> IntoRoot<(Symbol, RootObj)> for (Symbol, GcObj<'ob>) {
    unsafe fn into_root(self) -> (Symbol, RootObj) {
        (self.0, RootObj::new(self.1))
    }
}

impl<T: IntoRoot<U>, U> IntoRoot<Vec<U>> for Vec<T> {
    unsafe fn into_root(self) -> Vec<U> {
        self.into_iter().map(|x| x.into_root()).collect()
    }
}

#[repr(transparent)]
#[derive(Default)]
pub(crate) struct RootObj {
    obj: RawObj,
}

impl RootObj {
    pub(crate) fn new(obj: GcObj) -> Self {
        Self {
            obj: obj.into_raw(),
        }
    }
}

impl Trace for RootObj {
    fn mark(&self, stack: &mut Vec<RawObj>) {
        let obj = unsafe { GcObj::from_raw(self.obj) };
        obj.trace_mark(stack);
    }
}

impl<T> Trace for Gc<T> {
    fn mark(&self, stack: &mut Vec<RawObj>) {
        self.as_obj().trace_mark(stack);
    }
}

impl Debug for RootObj {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(unsafe { &GcObj::from_raw(self.obj) }, f)
    }
}

#[repr(transparent)]
#[derive(Debug)]
pub(crate) struct RootCons {
    obj: *const Cons,
}

impl RootCons {
    pub(crate) fn new(obj: &Cons) -> Self {
        Self {
            obj: unsafe { std::mem::transmute::<&Cons, *const Cons>(obj) },
        }
    }
}

impl Trace for RootCons {
    fn mark(&self, stack: &mut Vec<RawObj>) {
        unsafe {
            (*self.obj).mark(stack);
        }
    }
}

pub(crate) struct RootStruct<'rt> {
    set: bool,
    root_set: &'rt RootSet,
}

impl<'rt> Debug for RootStruct<'rt> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcRoot")
            .field("root_set", &self.root_set)
            .finish()
    }
}

impl<'rt> Drop for RootStruct<'rt> {
    fn drop(&mut self) {
        assert!(self.set, "RootStruct was dropped while still not set");
        self.root_set.root_structs.borrow_mut().pop();
    }
}

impl<'rt> RootStruct<'rt> {
    pub(crate) unsafe fn new(root_set: &'rt RootSet) -> Self {
        Self {
            set: false,
            root_set,
        }
    }

    pub(crate) fn set<'a, 'id, T: Trace + 'static>(
        &mut self,
        root: &'a mut Root<'id, T>,
    ) -> &'a Root<'id, T> {
        assert!(!self.set, "RootStruct should only be set once");
        let dyn_ptr = root.deref() as &dyn Trace as *const dyn Trace;
        self.set = true;
        self.root_set.root_structs.borrow_mut().push(dyn_ptr);
        unsafe { &*dyn_ptr.cast::<Root<'id, T>>() }
    }

    pub(crate) fn set_rt<T: Trace>(&mut self, root: &mut Rt<T>) -> &Rt<T> {
        assert!(!self.set, "RootStruct should only be set once");
        let dyn_ptr = &root.inner as &dyn Trace as *const dyn Trace;
        let dyn_ptr: *const (dyn Trace + 'static) = unsafe { std::mem::transmute(dyn_ptr) };
        self.set = true;
        self.root_set.root_structs.borrow_mut().push(dyn_ptr);
        unsafe { &*dyn_ptr.cast::<Rt<T>>() }
    }
}

pub(crate) struct RootOwner<'id>(LCellOwner<'id>);

impl<'id> RootOwner<'id> {
    pub(crate) fn new(guard: generativity::Guard<'id>) -> Self {
        Self(LCellOwner::new(guard))
    }
}

#[repr(transparent)]
pub(crate) struct Root<'id, T: ?Sized>(LCell<'id, Rt<T>>);

impl<'id, T> Root<'id, T> {
    /// Create a new Root
    ///
    /// # SAFETY
    ///
    /// This method is only safe to call if Root never moves and drops in stack
    /// order. Use the [`root_struct`] macro.
    pub(crate) unsafe fn new(obj: T) -> Self
    where
        T: 'static,
    {
        Root(LCell::new(Rt::new(obj)))
    }

    pub(super) fn deref(&mut self) -> &T {
        // SAFETY: if we have a &mut self, we know that there are no other
        // owners, so we don't need RootOwner. And we can cast since LCell is
        // repr(transparent)
        // TODO: LCell is not transparent yet. See https://github.com/uazu/qcell/pull/36
        unsafe { &*addr_of!(self.0).cast::<T>() }
    }

    pub(crate) fn borrow<'a>(&'a self, owner: &'a RootOwner<'id>) -> &'a Rt<T> {
        owner.0.ro(&self.0)
    }

    pub(crate) fn borrow_mut<'a>(
        &'a self,
        owner: &'a mut RootOwner<'id>,
        _: &'a Arena,
    ) -> &'a mut Rt<T> {
        owner.0.rw(&self.0)
    }

    pub(crate) fn borrow_mut2<'a, U>(
        gc1: &'a Self,
        gc2: &'a Root<'id, U>,
        owner: &'a mut RootOwner<'id>,
        _: &'a Arena,
    ) -> (&'a mut Rt<T>, &'a mut Rt<U>) {
        owner.0.rw2(&gc1.0, &gc2.0)
    }

    pub(crate) unsafe fn borrow_mut_unchecked2<'a, U>(
        gc1: &'a Self,
        gc2: &'a Root<'id, U>,
        owner: &'a mut RootOwner<'id>,
    ) -> (&'a mut Rt<T>, &'a mut Rt<U>) {
        owner.0.rw2(&gc1.0, &gc2.0)
    }
}

#[macro_export]
macro_rules! root_struct {
    ($ident:ident, $value:expr, $arena:ident) => {
        let mut $ident = unsafe { $crate::core::arena::Root::new($value) };
        let mut root = unsafe { $crate::core::arena::RootStruct::new($arena.get_root_set()) };
        let $ident = root.set(&mut $ident);
    };
}

#[macro_export]
macro_rules! rootx {
    ($ident:ident, $arena:ident) => {
        let mut x = unsafe {
            $crate::core::arena::Rt::new($crate::core::object::WithLifetime::with_lifetime($ident))
        };
        let mut root = unsafe { $crate::core::arena::RootStruct::new($arena.get_root_set()) };
        let $ident = root.set_rt(&mut x);
    };
}

// TODO: see if this type can be made local
#[repr(transparent)]
pub(crate) struct Rt<T: ?Sized> {
    inner: T,
}

impl<T: ?Sized + Debug> Debug for Rt<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self.inner, f)
    }
}

impl<T: PartialEq> PartialEq<T> for Rt<T> {
    fn eq(&self, other: &T) -> bool {
        self.inner == *other
    }
}

impl<T> AsRef<T> for Rt<T> {
    fn as_ref(&self) -> &T {
        &self.inner
    }
}

impl<T> Rt<T> {
    pub(crate) unsafe fn new(data: T) -> Self
    where
        T: 'static,
    {
        Rt { inner: data }
    }

    pub(crate) fn getx<'ob, U>(&self, _: &'ob Arena) -> U
    where
        T: WithLifetime<'ob, Out = U> + Copy,
    {
        unsafe { self.inner.with_lifetime() }
    }
}

impl Rt<RootObj> {
    pub(crate) fn set(&mut self, item: GcObj<'_>) {
        self.inner.obj = item.into_raw();
    }

    pub(crate) fn obj(&self) -> GcObj {
        unsafe { GcObj::from_raw(self.inner.obj) }
    }

    pub(crate) fn bind<'ob>(&self, gc: &'ob Arena) -> GcObj<'ob> {
        unsafe { gc.bind(GcObj::from_raw(self.inner.obj)) }
    }
}

impl Deref for Rt<RootObj> {
    type Target = RootObj;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'ob> AsRef<GcObj<'ob>> for Rt<RootObj> {
    fn as_ref(&self) -> &GcObj<'ob> {
        unsafe { &*(self as *const Self).cast::<GcObj>() }
    }
}

impl<'ob> AsRef<[GcObj<'ob>]> for Rt<[RootObj]> {
    fn as_ref(&self) -> &[GcObj<'ob>] {
        let ptr = self.inner.as_ptr().cast::<GcObj>();
        let len = self.inner.len();
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}

impl Rt<RootCons> {
    pub(crate) fn set(&mut self, item: &Cons) {
        self.inner.obj = unsafe { std::mem::transmute(item) }
    }
}

impl Deref for Rt<RootCons> {
    type Target = Cons;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.inner.obj }
    }
}

impl AsRef<Cons> for Rt<RootCons> {
    fn as_ref(&self) -> &Cons {
        unsafe { &*(self as *const Self).cast::<Cons>() }
    }
}

impl AsRef<[Cons]> for Rt<[RootCons]> {
    fn as_ref(&self) -> &[Cons] {
        let ptr = self.inner.as_ptr().cast::<Cons>();
        let len = self.inner.len();
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}

impl<T, U> Deref for Rt<(T, U)> {
    type Target = (Rt<T>, Rt<U>);

    fn deref(&self) -> &Self::Target {
        unsafe { &*(self as *const Rt<(T, U)>).cast::<(Rt<T>, Rt<U>)>() }
    }
}

impl<T, U> DerefMut for Rt<(T, U)> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *(self as *mut Rt<(T, U)>).cast::<(Rt<T>, Rt<U>)>() }
    }
}

impl<T> Deref for Rt<Option<T>> {
    type Target = Option<Rt<T>>;

    fn deref(&self) -> &Self::Target {
        unsafe { &*(self as *const Rt<Option<T>>).cast::<Option<Rt<T>>>() }
    }
}

impl<T> DerefMut for Rt<Option<T>> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *(self as *mut Rt<Option<T>>).cast::<Option<Rt<T>>>() }
    }
}

impl Rt<Option<RootObj>> {
    pub(crate) fn set(&mut self, obj: GcObj) {
        self.inner = Some(RootObj::new(obj));
    }
}

impl<T, I: SliceIndex<[T]>> Index<I> for Rt<Vec<T>> {
    type Output = Rt<I::Output>;

    fn index(&self, index: I) -> &Self::Output {
        unsafe { &*(Index::index(&self.inner, index) as *const I::Output as *const Rt<I::Output>) }
    }
}

impl<T, I: SliceIndex<[T]>> IndexMut<I> for Rt<Vec<T>> {
    fn index_mut(&mut self, index: I) -> &mut Self::Output {
        unsafe {
            &mut *(IndexMut::index_mut(&mut self.inner, index) as *mut I::Output
                as *mut Rt<I::Output>)
        }
    }
}

impl<T, I: SliceIndex<[T]>> Index<I> for Rt<[T]> {
    type Output = Rt<I::Output>;

    fn index(&self, index: I) -> &Self::Output {
        unsafe { &*(Index::index(&self.inner, index) as *const I::Output as *const Rt<I::Output>) }
    }
}

impl<T, I: SliceIndex<[T]>> IndexMut<I> for Rt<[T]> {
    fn index_mut(&mut self, index: I) -> &mut Self::Output {
        unsafe {
            &mut *(IndexMut::index_mut(&mut self.inner, index) as *mut I::Output
                as *mut Rt<I::Output>)
        }
    }
}

impl<T> Rt<Vec<T>> {
    pub(crate) fn as_slice(&self) -> &[Rt<T>] {
        // SAFETY: `Gc<T>` has the same memory layout as `T`.
        unsafe { &*(self.inner.as_slice() as *const [T] as *const [Rt<T>]) }
    }

    pub(crate) fn as_gc(&self) -> &Rt<[T]> {
        // SAFETY: `Gc<T>` has the same memory layout as `T`.
        unsafe { &*(self.inner.as_slice() as *const [T] as *const Rt<[T]>) }
    }

    pub(crate) fn as_mut_slice(&mut self) -> &mut [Rt<T>] {
        // SAFETY: `Gc<T>` has the same memory layout as `T`.
        unsafe { &mut *(self.inner.as_mut_slice() as *mut [T] as *mut [Rt<T>]) }
    }

    pub(crate) fn push<U: IntoRoot<T>>(&mut self, item: U) {
        self.inner.push(unsafe { item.into_root() });
    }
    pub(crate) fn truncate(&mut self, len: usize) {
        self.inner.truncate(len);
    }

    pub(crate) fn append(&mut self, other: &mut Self) {
        self.inner.append(&mut other.inner);
    }

    pub(crate) fn clear(&mut self) {
        self.inner.clear();
    }
}

impl<T> Deref for Rt<Vec<T>> {
    type Target = [Rt<T>];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<T> DerefMut for Rt<Vec<T>> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl<K, V> Rt<HashMap<K, V>>
where
    K: Eq + std::hash::Hash,
{
    pub(crate) fn get<Q: ?Sized>(&self, k: &Q) -> Option<&Rt<V>>
    where
        K: std::borrow::Borrow<Q>,
        Q: std::hash::Hash + Eq,
    {
        self.inner
            .get(k)
            .map(|v| unsafe { &*(v as *const V).cast::<Rt<V>>() })
    }

    pub(crate) fn get_mut<Q: ?Sized>(&mut self, k: &Q) -> Option<&mut Rt<V>>
    where
        K: std::borrow::Borrow<Q>,
        Q: std::hash::Hash + Eq,
    {
        self.inner
            .get_mut(k)
            .map(|v| unsafe { &mut *(v as *mut V).cast::<Rt<V>>() })
    }

    pub(crate) fn insert<R: IntoRoot<V>>(&mut self, k: K, v: R) {
        self.inner.insert(k, unsafe { v.into_root() });
    }
}

type Prop = Rt<HashMap<Symbol, Vec<(Symbol, RootObj)>>>;
impl Rt<Environment> {
    pub(crate) fn vars(&self) -> &Rt<HashMap<Symbol, RootObj>> {
        unsafe { &*addr_of!(self.inner.vars).cast() }
    }

    pub(crate) fn vars_mut(&mut self) -> &mut Rt<HashMap<Symbol, RootObj>> {
        unsafe { &mut *addr_of_mut!(self.inner.vars).cast() }
    }

    pub(crate) fn props(&self) -> &Prop {
        unsafe { &*addr_of!(self.inner.props).cast() }
    }

    pub(crate) fn props_mut(&mut self) -> &mut Prop {
        unsafe { &mut *addr_of_mut!(self.inner.props).cast() }
    }
}

#[cfg(test)]
mod test {
    use super::super::super::object::Object;
    use super::super::RootSet;
    use super::*;

    #[test]
    fn indexing() {
        let root = &RootSet::default();
        let arena = &Arena::new(root);
        let mut vec: Rt<Vec<RootObj>> = Rt { inner: vec![] };

        vec.push(GcObj::NIL);
        assert!(matches!(vec[0].obj().get(), Object::Nil));
        let str1 = arena.add("str1");
        let str2 = arena.add("str2");
        vec.push(str1);
        vec.push(str2);
        let slice = &vec[0..3];
        assert_eq!(vec![GcObj::NIL, str1, str2], slice.as_ref());
    }
}
