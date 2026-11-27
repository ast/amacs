//! The heap: where objects live, and the collector that frees those that
//! can no longer be reached.
//!
//! Objects are slots in a vector and a [`Handle`] is an index, so there is
//! no unsafe code here. Collection is mark and sweep. It only happens when
//! the machine asks for it between instructions, so code written in Rust
//! can hold handles in local variables without telling anyone.

use std::rc::Rc;

use crate::code::Proto;
use crate::value::{Handle, Object, Value};

/// Objects allocated before the first collection is considered.
const FIRST_THRESHOLD: usize = 64 * 1024;

#[derive(Default)]
pub struct Heap {
    slots: Vec<Option<Object>>,
    free: Vec<u32>,
    /// The slots that hold objects, in no order. The collector goes
    /// through these and not through all slots, so that what it costs
    /// goes by the objects there are, not by how many there once were.
    used: Vec<u32>,
    marks: Vec<bool>,
    /// Objects allocated since the last collection.
    allocated: usize,
    threshold: usize,
    /// Counts collections, to mark shared code only once in each.
    epoch: u64,
    pub collections: u64,
}

/// Collects the objects that are reachable, starting from the roots it is
/// given.
pub struct Marker<'a> {
    slots: &'a [Option<Object>],
    marks: &'a mut [bool],
    epoch: u64,
    pending: Vec<Handle>,
}

impl Marker<'_> {
    pub fn value(&mut self, value: Value) {
        if let Value::Obj(handle) = value {
            self.handle(handle);
        }
    }

    pub fn values(&mut self, values: &[Value]) {
        for value in values {
            self.value(*value);
        }
    }

    pub fn handle(&mut self, handle: Handle) {
        let index = handle.0 as usize;
        if !self.marks[index] {
            self.marks[index] = true;
            self.pending.push(handle);
        }
    }

    /// Marks the constants of a piece of code and of the code inside it.
    pub fn proto(&mut self, proto: &Rc<Proto>) {
        if proto.epoch.get() == self.epoch {
            return;
        }
        proto.epoch.set(self.epoch);
        self.values(&proto.constants);
        for inner in &proto.protos {
            self.proto(inner);
        }
    }

    /// Marks everything reachable from what has been marked so far.
    fn trace(&mut self) {
        let slots = self.slots;
        while let Some(handle) = self.pending.pop() {
            let Some(object) = &slots[handle.0 as usize] else {
                continue;
            };
            match object {
                Object::Pair(car, cdr) => {
                    self.value(*car);
                    self.value(*cdr);
                }
                Object::Vector(items) | Object::Values(items) => self.values(items),
                Object::Closure(closure) => {
                    self.values(&closure.captured);
                    self.proto(&closure.proto);
                }
                Object::Cell(value) => self.value(*value),
                Object::Escape(tag, value) => {
                    self.value(*tag);
                    self.value(*value);
                }
                Object::Table(table) => {
                    for (key, value) in table.entries.values() {
                        self.value(*key);
                        self.value(*value);
                    }
                }
                Object::Record { kind, fields } => {
                    self.handle(*kind);
                    self.values(fields);
                }
                Object::Parameter { value, converter } => {
                    self.value(*value);
                    self.value(*converter);
                }
                Object::Fiber(fiber) => fiber.mark(self),
                Object::Error(error) => self.values(&error.irritants),
                Object::Str(_)
                | Object::Native(_)
                | Object::RecordType { .. }
                | Object::Foreign(_) => {}
            }
        }
    }
}

impl Heap {
    pub fn new() -> Self {
        Self {
            threshold: FIRST_THRESHOLD,
            ..Self::default()
        }
    }

    pub fn alloc(&mut self, object: Object) -> Handle {
        self.allocated += 1;
        let index = match self.free.pop() {
            Some(index) => {
                self.slots[index as usize] = Some(object);
                index
            }
            None => {
                self.slots.push(Some(object));
                self.marks.push(false);
                self.slots.len() as u32 - 1
            }
        };
        self.used.push(index);
        Handle(index)
    }

    /// The object a handle refers to.
    ///
    /// # Panics
    /// If the object has been collected, which means a root was missed.
    pub fn get(&self, handle: Handle) -> &Object {
        self.slots[handle.0 as usize]
            .as_ref()
            .expect("a live handle refers to an object")
    }

    pub fn get_mut(&mut self, handle: Handle) -> &mut Object {
        self.slots[handle.0 as usize]
            .as_mut()
            .expect("a live handle refers to an object")
    }

    /// Whether enough has been allocated to make a collection worthwhile.
    pub fn wants_collection(&self) -> bool {
        self.allocated >= self.threshold
    }

    /// Number of objects: those alive at the last collection and what has
    /// been allocated since.
    pub fn len(&self) -> usize {
        self.used.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Frees every object that cannot be reached from the roots, which
    /// `mark_roots` marks.
    pub fn collect(&mut self, mark_roots: impl FnOnce(&mut Marker)) {
        self.epoch += 1;
        self.collections += 1;
        let mut marker = Marker {
            slots: &self.slots,
            marks: &mut self.marks,
            epoch: self.epoch,
            pending: Vec::new(),
        };
        mark_roots(&mut marker);
        marker.trace();

        let Self {
            slots,
            free,
            used,
            marks,
            ..
        } = self;
        used.retain(|index| {
            let live = std::mem::take(&mut marks[*index as usize]);
            if !live {
                slots[*index as usize] = None;
                free.push(*index);
            }
            live
        });
        let live = used.len();
        self.allocated = 0;
        // Collect again when twice as much as is alive has been allocated,
        // so that the work of collecting stays in proportion.
        self.threshold = (live * 2).max(FIRST_THRESHOLD);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_collection_frees_what_is_not_reached_and_keeps_the_rest() {
        let mut heap = Heap::new();
        let kept = heap.alloc(Object::Str("kept".into()));
        let pair = heap.alloc(Object::Pair(Value::Obj(kept), Value::Nil));
        for _ in 0..1000 {
            heap.alloc(Object::Pair(Value::Nil, Value::Nil));
        }
        assert_eq!(heap.len(), 1002);
        heap.collect(|marker| marker.handle(pair));
        assert_eq!(heap.len(), 2);
        assert!(matches!(heap.get(kept), Object::Str(text) if text == "kept"));

        // The freed slots are used again, and a second collection finds
        // the same.
        let slots = heap.slots.len();
        for _ in 0..1000 {
            heap.alloc(Object::Pair(Value::Nil, Value::Nil));
        }
        assert_eq!(heap.slots.len(), slots);
        heap.collect(|marker| marker.handle(pair));
        assert_eq!(heap.len(), 2);
        heap.collect(|_| {});
        assert!(heap.is_empty());
    }
}
