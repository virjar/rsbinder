// Copyright 2022 Jeff Kim <hiking90@gmail.com>
// SPDX-License-Identifier: Apache-2.0

use std::any::Any;

use crate::{
    parcel::*,
    binder::*,
    error::*,
    parcelable::*,
    thread_state,
};


#[derive(Debug, Clone, PartialEq)]
pub struct ProxyHandle {
    handle: u32,
    descriptor: String,
}

impl ProxyHandle {
    pub fn new(handle: u32, interface: String) -> Box<Self> {
        Box::new(Self {
            handle,
            descriptor: interface,
        })
    }

    pub fn handle(&self) -> u32 {
        self.handle
    }

    pub fn descriptor(&self) -> &str {
        &self.descriptor
    }

    pub fn submit_transact(&self, code: TransactionCode, data: &Parcel, flags: TransactionFlags) -> Result<Option<Parcel>> {
        thread_state::transact(self.handle, code, data, flags)
    }

    pub fn prepare_transact(&self, write_header: bool) -> Result<Parcel> {
        let mut data = Parcel::new();

        if write_header == true {
            data.write_interface_token(String16(self.descriptor.to_owned()))?;
        }

        Ok(data)
    }
}

impl IBinder for ProxyHandle {
    fn link_to_death(&mut self, _recipient: &mut dyn DeathRecipient) -> Result<()> {
        todo!("IBinder for Proxy<I> - link_to_death")
    }

    /// Remove a previously registered death notification.
    /// The recipient will no longer be called if this object
    /// dies.
    fn unlink_to_death(&mut self, _recipient: &mut dyn DeathRecipient) -> Result<()> {
        todo!("IBinder for Proxy<I> - unlink_to_death")
    }

    /// Send a ping transaction to this object
    fn ping_binder(&self) -> Result<()> {
        thread_state::ping_binder(self.handle)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn is_remote(&self) -> bool {
        true
    }
}

pub trait Proxy : Sized + Interface {
    /// The Binder interface descriptor string.
    ///
    /// This string is a unique identifier for a Binder interface, and should be
    /// the same between all implementations of that interface.
    fn descriptor() -> &'static str;

    /// Create a new interface from the given proxy, if it matches the expected
    /// type of this interface.
    fn from_binder(binder: StrongIBinder) -> Result<Self>;
}


// /// # Safety
// ///
// /// An `RawIBinder` is an immutable handle to a C++ IBinder, which is thread-safe
// unsafe impl Send for RawIBinder {}

// /// # Safety
// ///
// /// An `RawIBinder` is an immutable handle to a C++ IBinder, which is thread-safe
// unsafe impl Sync for RawIBinder {}

// impl RawIBinder {
//     /// Create an `RawIBinder` wrapper object from a raw `AIBinder` pointer.
//     ///
//     /// # Safety
//     ///
//     /// This constructor is safe iff `ptr` is a null pointer or a valid pointer
//     /// to an `AIBinder`.
//     ///
//     /// In the non-null case, this method conceptually takes ownership of a strong
//     /// reference to the object, so `AIBinder_incStrong` must have been called
//     /// on the pointer before passing it to this constructor. This is generally
//     /// done by Binder NDK methods that return an `AIBinder`, but care should be
//     /// taken to ensure this invariant.
//     ///
//     /// All `RawIBinder` objects that are constructed will hold a valid pointer
//     /// to an `AIBinder`, which will remain valid for the entire lifetime of the
//     /// `RawIBinder` (we keep a strong reference, and only decrement on drop).
//     pub(crate) unsafe fn from_raw(ptr: *mut sys::AIBinder) -> Option<Self> {
//         ptr::NonNull::new(ptr).map(Self)
//     }

//     /// Extract a raw `AIBinder` pointer from this wrapper.
//     ///
//     /// This method should _only_ be used for testing. Do not try to use the NDK
//     /// interface directly for anything else.
//     ///
//     /// # Safety
//     ///
//     /// The resulting pointer is valid only as long as the RawIBinder is alive.
//     /// The RawIBinder object retains ownership of the AIBinder and the caller
//     /// should not attempt to free the returned pointer.
//     pub unsafe fn as_raw(&self) -> *mut sys::AIBinder {
//         self.0.as_ptr()
//     }

//     /// Return true if this binder object is hosted in a different process than
//     /// the current one.
//     pub fn is_remote(&self) -> bool {
//         unsafe {
//             // Safety: `RawIBinder` guarantees that it always contains a valid
//             // `AIBinder` pointer.
//             sys::AIBinder_isRemote(self.as_native())
//         }
//     }

//     /// Try to convert this Binder object into a trait object for the given
//     /// Binder interface.
//     ///
//     /// If this object does not implement the expected interface, the error
//     /// `StatusCode::BAD_TYPE` is returned.
//     pub fn into_interface<I: FromIBinder + Interface + ?Sized>(self) -> Result<Strong<I>> {
//         FromIBinder::try_from(self)
//     }

//     /// Return the interface class of this binder object, if associated with
//     /// one.
//     pub fn get_class(&mut self) -> Option<InterfaceClass> {
//         unsafe {
//             // Safety: `RawIBinder` guarantees that it always contains a valid
//             // `AIBinder` pointer. `AIBinder_getClass` returns either a null
//             // pointer or a valid pointer to an `AIBinder_Class`. After mapping
//             // null to None, we can safely construct an `InterfaceClass` if the
//             // pointer was non-null.
//             let class = sys::AIBinder_getClass(self.as_native_mut());
//             class.as_ref().map(|p| InterfaceClass::from_ptr(p))
//         }
//     }

//     /// Creates a new weak reference to this binder object.
//     pub fn downgrade(&mut self) -> WpIBinder {
//         WpIBinder::new(self)
//     }
// }
