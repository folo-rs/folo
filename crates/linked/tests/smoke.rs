//! Basic operations on linked objects.

use std::{
    sync::{Arc, Mutex},
    thread,
};

use linked::Object;

#[test]
fn linked_objects_smoke_test() {
    #[linked::object]
    struct Thing {
        local_value: usize,
        global_value: Arc<Mutex<String>>,
    }

    impl Thing {
        fn new(local_value: usize, global_value: String) -> Self {
            let global_value = Arc::new(Mutex::new(global_value));

            linked::new!(Self {
                local_value,
                global_value: Arc::clone(&global_value),
            })
        }

        fn set_global_value(&self, value: &str) {
            let mut global_value = self.global_value.lock().unwrap();
            *global_value = value.to_string();
        }

        fn get_global_value(&self) -> String {
            let global_value = self.global_value.lock().unwrap();
            global_value.clone()
        }

        fn get_local_value(&self) -> usize {
            self.local_value
        }

        fn set_local_value(&mut self, value: usize) {
            self.local_value = value;
        }
    }

    let mut linked_object = Thing::new(42, "hello".to_string());

    assert_eq!(linked_object.get_local_value(), 42);
    assert_eq!(linked_object.get_global_value(), "hello");

    let clone = linked_object.clone();

    linked_object.set_global_value("world");
    linked_object.set_local_value(43);

    assert_eq!(linked_object.get_local_value(), 43);
    assert_eq!(linked_object.get_global_value(), "world");

    assert_eq!(clone.get_local_value(), 42);
    assert_eq!(clone.get_global_value(), "world");

    let handle = linked_object.family();

    thread::spawn(move || {
        let mut linked_object: Thing = handle.into();

        assert_eq!(linked_object.get_local_value(), 42);
        assert_eq!(linked_object.get_global_value(), "world");

        linked_object.set_global_value("paradise");
        linked_object.set_local_value(45);
    })
    .join()
    .unwrap();

    assert_eq!(linked_object.get_local_value(), 43);
    assert_eq!(linked_object.get_global_value(), "paradise");

    assert_eq!(clone.get_local_value(), 42);
    assert_eq!(clone.get_global_value(), "paradise");
}
