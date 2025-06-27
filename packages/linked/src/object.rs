// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

use crate::Family;

/// Operations available on every instance of a [linked object][crate].
///
/// The only supported way to implement this is via [`#[linked::object]`][crate::object].
pub trait Object: From<Family<Self>> + Sized + Clone + 'static {
    /// The object family that the current instance is linked to.
    ///
    /// The returned object can be used to create additional instances linked to the same family.
    ///
    /// # Example
    ///
    /// ```
    /// # use std::sync::{Arc, Mutex};
    /// #
    /// # #[linked::object]
    /// # struct SharedResource {
    /// #     data: Arc<Mutex<String>>,
    /// # }
    /// #
    /// # impl SharedResource {
    /// #     pub fn new(initial_data: String) -> Self {
    /// #         let data = Arc::new(Mutex::new(initial_data));
    /// #         linked::new!(Self {
    /// #             data: Arc::clone(&data),
    /// #         })
    /// #     }
    /// #
    /// #     pub fn get_data(&self) -> String {
    /// #         self.data.lock().unwrap().clone()
    /// #     }
    /// #
    /// #     pub fn set_data(&self, new_data: String) {
    /// #         *self.data.lock().unwrap() = new_data;
    /// #     }
    /// # }
    /// use std::thread;
    ///
    /// use linked::Object; // Import trait to access .family() method
    ///
    /// let resource = SharedResource::new("initial".to_string());
    ///
    /// // Get the family handle from an existing instance
    /// let family = resource.family();
    ///
    /// // Use the family to create a new instance on another thread
    /// thread::spawn(move || {
    ///     let new_resource: SharedResource = family.into();
    ///
    ///     // This new instance shares state with the original
    ///     new_resource.set_data("updated".to_string());
    /// })
    /// .join()
    /// .unwrap();
    ///
    /// // The original instance sees the update
    /// assert_eq!(resource.get_data(), "updated");
    /// ```
    fn family(&self) -> Family<Self>;
}
