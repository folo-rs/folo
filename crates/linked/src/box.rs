// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

use std::boxed::Box as StdBox;
use std::ops::{Deref, DerefMut};

/// A linked object that acts like a `std::boxed::Box<dyn MyTrait>`.
///
/// Intended to represent linked instances of `T` where `T: MyTrait`. This is for use with types
/// that are always exposed to user code as trait objects via `linked::Box<dyn MyTrait>`.
///
/// The `Box` itself implements the linked object API from [`#[linked::object]`][3]. The type
/// `T` is not decorated with the [`#[linked::object]`][3] attribute when using `Box` and simply
/// uses linked object patterns in its constructor.
///
/// # Usage
///
/// Use it like a regular `std::boxed::Box<T>` that also happens to support the linked object
/// patterns. The box can be used via all the standard mechanisms such as:
///
/// * [`linked::instances!`][1]
/// * [`linked::thread_local_rc!`][2]
/// * [`linked::thread_local_arc!`][4] (if `T: Sync`)
/// * [`linked::InstancePerThread<T>`][5]
/// * [`linked::InstancePerThreadSync<T>`][6] (if `T: Sync`)
/// * [`linked::Family<T>`][7]
///
/// # Implementation
///
/// Instead of a typical constructor, create one that returns `linked::Box<dyn MyTrait>`. Inside
/// this constructor, create a `linked::Box` instance using the [`linked::new_box!` macro][8].
/// The first macro parameter is the type of the trait object inside the box, and the second is a
/// `Self` struct-expression to create one linked instance of the implementation type.
///
/// ```
/// # trait ConfigSource {}
/// # impl ConfigSource for XmlConfig {}
/// // If using linked::Box, do not put `#[linked::object]` on the struct.
/// // The linked::Box itself is the linked object and our struct is only its contents.
/// struct XmlConfig {
///     config: String,
/// }
///
/// impl XmlConfig {
///     pub fn new_as_config_source() -> linked::Box<dyn ConfigSource> {
///         // Constructing instances works logically the same as for regular linked objects.
///         //
///         // The only differences are:
///         // 1. We use `linked::new_box!` instead of `linked::new!`
///         // 2. There is an additional parameter to the macro to name the trait object type
///         linked::new_box!(
///             dyn ConfigSource,
///             Self {
///                 config: "xml".to_string(),
///             }
///         )
///     }
/// }
/// ```
///
/// Any connections between the instances should be established via the captured state of this
/// closure (e.g. sharing an `Arc` or setting up messaging channels).
///
/// # Example
///
/// Using the linked objects as `linked::Box<dyn ConfigSource>`, without the user code knowing the
/// exact type of the object in the box:
///
/// ```
/// trait ConfigSource {
///     fn config(&self) -> String;
/// }
///
/// struct XmlConfig {}
/// struct IniConfig {}
///
/// impl ConfigSource for XmlConfig {
///     fn config(&self) -> String {
///         "xml".to_string()
///     }
/// }
///
/// impl ConfigSource for IniConfig {
///     fn config(&self) -> String {
///         "ini".to_string()
///     }
/// }
///
/// impl XmlConfig {
///     pub fn new_as_config_source() -> linked::Box<dyn ConfigSource> {
///         linked::new_box!(dyn ConfigSource, XmlConfig {})
///     }
/// }
///
/// impl IniConfig {
///     pub fn new_as_config_source() -> linked::Box<dyn ConfigSource> {
///         linked::new_box!(dyn ConfigSource, IniConfig {})
///     }
/// }
///
/// let xml_config = XmlConfig::new_as_config_source();
/// let ini_config = IniConfig::new_as_config_source();
///
/// let configs: [linked::Box<dyn ConfigSource>; 2] = [xml_config, ini_config];
///
/// assert_eq!(configs[0].config(), "xml".to_string());
/// assert_eq!(configs[1].config(), "ini".to_string());
/// ```
///
/// [1]: crate::instances
/// [2]: crate::thread_local_rc
/// [3]: crate::object
/// [4]: crate::thread_local_arc
/// [5]: crate::InstancePerThread
/// [6]: crate::InstancePerThreadSync
/// [7]: crate::Family
/// [8]: crate::new_box
#[linked::object]
#[derive(Debug)]
pub struct Box<T: ?Sized + 'static> {
    value: StdBox<T>,
}

impl<T: ?Sized> Box<T> {
    /// This is an implementation detail of the `linked::new_box!` macro and is not part of the
    /// public API. It is not meant to be used directly and may change or be removed at any time.
    #[doc(hidden)]
    #[must_use]
    pub fn new(instance_factory: impl Fn() -> StdBox<T> + Send + Sync + 'static) -> Self {
        linked::new!(Self {
            value: (instance_factory)(),
        })
    }
}

impl<T: ?Sized + 'static> Deref for Box<T> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T: ?Sized + 'static> DerefMut for Box<T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}

/// Defines the template used to create every instance in a `linked::Box<T>` object family.
///
/// This macro is meant to be used in the context of creating a new linked instance of
/// `T` that is meant to be always expressed via an abstraction (`dyn SomeTrait`).
///
/// # Arguments
///
/// * `$dyn_trait` - The trait object that the linked object is to be used as (e.g. `dyn SomeTrait`).
/// * `$ctor` - The Self-expression that serves as the template for constructing new linked
///   instances on demand. This will move-capture any referenced state. All captured
///   values must be thread-safe (`Send` + `Sync` + `'static`).
///
/// # Example
///
/// ```rust
/// # trait ConfigSource {}
/// # impl ConfigSource for XmlConfig {}
/// // If using linked::Box, do not put `#[linked::object]` on the struct.
/// // The linked::Box itself is the linked object and our struct is only its contents.
/// struct XmlConfig {
///     config: String,
/// }
///
/// impl XmlConfig {
///     pub fn new_as_config_source() -> linked::Box<dyn ConfigSource> {
///         linked::new_box!(
///             dyn ConfigSource,
///             Self {
///                 config: "xml".to_string(),
///             }
///         )
///     }
/// }
/// ```
///
/// See `examples/linked_box.rs` for a complete example.
#[macro_export]
macro_rules! new_box {
    ($dyn_trait:ty, $ctor:expr) => {
        #[expect(trivial_casts, reason = "clearly expresses intent")]
        ::linked::Box::new(move || ::std::boxed::Box::new($ctor) as ::std::boxed::Box<$dyn_trait>)
    };
}

#[cfg(test)]
mod tests {
    use std::thread;

    use crate::Object;

    #[test]
    fn linked_box() {
        trait ConfigSource {
            fn config(&self) -> String;
            fn set_config(&mut self, config: String);
        }

        struct XmlConfig {
            config: String,
        }
        struct IniConfig {
            config: String,
        }

        impl ConfigSource for XmlConfig {
            fn config(&self) -> String {
                self.config.clone()
            }

            fn set_config(&mut self, config: String) {
                self.config = config;
            }
        }

        impl ConfigSource for IniConfig {
            fn config(&self) -> String {
                self.config.clone()
            }

            fn set_config(&mut self, config: String) {
                self.config = config;
            }
        }

        impl XmlConfig {
            fn new_as_config_source() -> linked::Box<dyn ConfigSource> {
                linked::new_box!(
                    dyn ConfigSource,
                    Self {
                        config: "xml".to_string(),
                    }
                )
            }
        }

        impl IniConfig {
            fn new_as_config_source() -> linked::Box<dyn ConfigSource> {
                linked::new_box!(
                    dyn ConfigSource,
                    Self {
                        config: "ini".to_string(),
                    }
                )
            }
        }

        let xml_config = XmlConfig::new_as_config_source();
        let ini_config = IniConfig::new_as_config_source();

        let mut configs = [xml_config, ini_config];

        assert_eq!(configs[0].config(), "xml".to_string());
        assert_eq!(configs[1].config(), "ini".to_string());

        configs[0].set_config("xml2".to_string());
        configs[1].set_config("ini2".to_string());

        assert_eq!(configs[0].config(), "xml2".to_string());
        assert_eq!(configs[1].config(), "ini2".to_string());

        let xml_config_family = configs[0].family();
        let ini_config_family = configs[1].family();

        thread::spawn(move || {
            let xml_config: linked::Box<dyn ConfigSource> = xml_config_family.into();
            let ini_config: linked::Box<dyn ConfigSource> = ini_config_family.into();

            // Note that the "config" is local to each instance, so it reset to the initial value.
            assert_eq!(xml_config.config(), "xml".to_string());
            assert_eq!(ini_config.config(), "ini".to_string());
        })
        .join()
        .unwrap();
    }
}
