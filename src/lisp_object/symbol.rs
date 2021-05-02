use crate::arena::Arena;
use crate::lisp_object::*;
use std::cmp;
use std::fmt;
use std::mem;
use std::sync::atomic::{AtomicI64, Ordering};

#[derive(Debug)]
pub struct InnerSymbol {
    name: String,
    func: FnCell,
}

#[derive(Debug)]
struct FnCell(AtomicI64);

impl FnCell {
    const fn new() -> Self {
        Self(AtomicI64::new(0))
    }

    fn set(&self, func: Function) {
        let value = unsafe { mem::transmute(func) };
        self.0.store(value, Ordering::Release);
    }

    fn get(&self) -> Option<Function> {
        let bits = self.0.load(Ordering::Acquire);
        match bits {
            0 => None,
            _ => Some(unsafe { mem::transmute(bits) }),
        }
    }
}

impl cmp::PartialEq for InnerSymbol {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(&*self, &*other)
    }
}

impl InnerSymbol {
    pub const fn new(name: String) -> Self {
        InnerSymbol {
            name,
            func: FnCell::new(),
        }
    }

    pub fn set_func(&self, func: Function) {
        self.func.set(func);
    }

    pub fn get_func(&self) -> Option<Function> {
        self.func.get()
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }
}

#[derive(Copy, Clone)]
pub struct Symbol(&'static InnerSymbol);
define_unbox!(Symbol);

impl Symbol {
    #[allow(clippy::missing_const_for_fn)]
    pub unsafe fn from_raw(ptr: *const InnerSymbol) -> Symbol {
        Symbol(&*ptr)
    }
}

impl fmt::Debug for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "'{}", &self.0.name)
    }
}

impl std::ops::Deref for Symbol {
    type Target = InnerSymbol;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<'obj> From<Symbol> for Object<'obj> {
    fn from(s: Symbol) -> Self {
        let ptr = s.0 as *const _;
        unsafe { Object::from_ptr(ptr as *mut u8, Tag::Symbol) }
    }
}

impl<'obj> IntoObject<'obj> for Symbol {
    fn into_object(self, _alloc: &Arena) -> (Object, bool) {
        (self.into(), false)
    }
}

impl<'obj> IntoTagObject<SymbolObject> for Symbol {
    fn into_object(self, _arena: &Arena) -> SymbolObject {
        let ptr = self.0 as *const _;
        SymbolObject(SymbolObject::new_tagged(ptr as i64))
    }
}

impl std::cmp::PartialEq for Symbol {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(&*self.0, &*other.0)
    }
}

impl std::cmp::Eq for Symbol {}

impl std::hash::Hash for Symbol {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let bits = (self.0 as *const _) as u64;
        bits.hash(state);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::error::Error;
    use crate::hashmap::HashMap;
    use crate::lisp_object::{FunctionValue, LispFn, SubrFn};

    #[test]
    fn size() {
        assert_eq!(32, std::mem::size_of::<InnerSymbol>());
        assert_eq!(8, std::mem::size_of::<Symbol>());
    }

    #[test]
    fn symbol_func() {
        let arena = Arena::new();
        let x = InnerSymbol::new("foo".to_owned());
        assert_eq!("foo", x.get_name());
        assert!(x.get_func().is_none());
        let func = LispFn::new(vec![1].into(), vec![], Arena::new(), 0, 0, false);
        x.set_func(arena.insert(func));
        let cell = x.get_func().unwrap();
        let before = match cell.val() {
            FunctionValue::LispFn(x) => x,
            _ => unreachable!(),
        };
        assert_eq!(before.op_codes.get(0).unwrap(), &1);
        let func = LispFn::new(vec![7].into(), vec![], Arena::new(), 0, 0, false);
        x.set_func(arena.insert(func));
        let cell = x.get_func().unwrap();
        let after = match cell.val() {
            FunctionValue::LispFn(x) => x,
            _ => unreachable!(),
        };
        assert_eq!(after.op_codes.get(0).unwrap(), &7);
        assert_eq!(before.op_codes.get(0).unwrap(), &1);
    }

    #[allow(clippy::clippy::unnecessary_wraps)]
    fn dummy<'obj>(
        vars: &[Object<'obj>],
        _map: &mut HashMap<Symbol, Object<'obj>>,
        _arena: &'obj Arena,
    ) -> Result<Object<'obj>, Error> {
        Ok(vars[0])
    }

    #[test]
    fn subr() {
        let arena = Arena::new();

        let sym = InnerSymbol::new("bar".to_owned());
        let core_func = SubrFn::new("bar", dummy, 0, 0, false);
        sym.set_func(arena.insert(core_func));

        match sym.get_func().unwrap().val() {
            FunctionValue::SubrFn(x) => {
                assert_eq!(*x, SubrFn::new("bar", dummy, 0, 0, false));
            }
            _ => unreachable!(),
        };
    }
}
