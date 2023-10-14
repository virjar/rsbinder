// Copyright 2022 Jeff Kim <hiking90@gmail.com>
// SPDX-License-Identifier: Apache-2.0

use std::sync::{Arc};
use std::os::unix::io::AsRawFd;
use std::cell::RefCell;
use log::error;

use crate::{
    parcel::*,
    error::*,
    binder::*,
    process_state::*,
    parcelable::*,
    sys::*,
};

thread_local! {
    static THREAD_STATE: RefCell<ThreadState> = RefCell::new(ThreadState::new());
}

const RETURN_STRINGS: [&str; 21] =
[
    "BR_ERROR",
    "BR_OK",
    "BR_TRANSACTION",
    "BR_REPLY",
    "BR_ACQUIRE_RESULT",
    "BR_DEAD_REPLY",
    "BR_TRANSACTION_COMPLETE",
    "BR_INCREFS",
    "BR_ACQUIRE",
    "BR_RELEASE",
    "BR_DECREFS",
    "BR_ATTEMPT_ACQUIRE",
    "BR_NOOP",
    "BR_SPAWN_LOOPER",
    "BR_FINISHED",
    "BR_DEAD_BINDER",
    "BR_CLEAR_DEATH_NOTIFICATION_DONE",
    "BR_FAILED_REPLY",
    "BR_FROZEN_REPLY",
    "BR_ONEWAY_SPAM_SUSPECT",
    "BR_TRANSACTION_SEC_CTX",
];

fn return_to_str(cmd: std::os::raw::c_uint) -> &'static str {
    let idx: usize = (cmd & binder::_IOC_NRMASK) as _;

    if idx < RETURN_STRINGS.len() {
        RETURN_STRINGS[idx]
    } else {
        "Unknown BR_ return"
    }
}

const COMMAND_STRINGS: [&str; 17] =
[
    "BC_TRANSACTION",
    "BC_REPLY",
    "BC_ACQUIRE_RESULT",
    "BC_FREE_BUFFER",
    "BC_INCREFS",
    "BC_ACQUIRE",
    "BC_RELEASE",
    "BC_DECREFS",
    "BC_INCREFS_DONE",
    "BC_ACQUIRE_DONE",
    "BC_ATTEMPT_ACQUIRE",
    "BC_REGISTER_LOOPER",
    "BC_ENTER_LOOPER",
    "BC_EXIT_LOOPER",
    "BC_REQUEST_DEATH_NOTIFICATION",
    "BC_CLEAR_DEATH_NOTIFICATION",
    "BC_DEAD_BINDER_DONE"
];

fn command_to_str(cmd: std::os::raw::c_uint) -> &'static str {
    let idx: usize = (cmd & 0xFF) as _;

    if idx < COMMAND_STRINGS.len() {
        COMMAND_STRINGS[idx]
    } else {
        "Unknown BC_ command"
    }
}

const WORK_SOURCE_PROPAGATED_BIT_INDEX: i64 = 32;
pub(crate) const UNSET_WORK_SOURCE: i32 = -1;

#[derive(Debug, Clone, Copy)]
struct TransactionState {
    calling_pid: binder::pid_t,
    calling_sid: *const u8,
    calling_uid: binder::uid_t,
    strict_mode_policy: i32,
    last_transaction_binder_flags: u32,
    work_source: binder::uid_t,
    propagate_work_source: bool,
}

impl TransactionState {
    fn from_transaction_data(data: &binder::binder_transaction_data_secctx) -> Self {
        TransactionState {
            calling_pid: data.transaction_data.sender_pid,
            calling_sid: data.secctx as _,
            calling_uid: data.transaction_data.sender_euid,
            strict_mode_policy: 0,
            last_transaction_binder_flags: data.transaction_data.flags,
            work_source: 0,
            propagate_work_source: false,
        }
    }
}


pub struct ThreadState {
    in_parcel: Parcel,
    out_parcel: Parcel,
    transaction: Option<TransactionState>,
    strict_mode_policy: i32,
    is_looper: bool,
    is_flushing: bool,
    post_strong_derefs: Vec<StrongIBinder>,
    post_weak_derefs: Vec<WeakIBinder>,
    call_restriction: CallRestriction,
}

impl ThreadState {
    fn new() -> Self {
        ThreadState {
            in_parcel: Parcel::new(),
            out_parcel: Parcel::new(),
            transaction: None,
            strict_mode_policy: 0,
            is_looper: false,
            is_flushing: false,
            post_strong_derefs: Vec::new(),
            post_weak_derefs: Vec::new(),
            call_restriction: ProcessState::as_self().call_restriction(),
        }
    }

    pub fn set_strict_mode_policy(&mut self, policy: i32) {
        self.strict_mode_policy = policy;
    }

    pub fn strict_mode_policy(&self) -> i32 {
        self.strict_mode_policy
    }

    pub fn last_transaction_binder_flags(&self) -> u32 {
        match self.transaction {
            Some(tr) => tr.last_transaction_binder_flags,
            None => 0,
        }
    }

    fn process_post_write_derefs(&mut self) {
        self.post_weak_derefs.clear();
        self.post_strong_derefs.clear();
    }

    fn clear_propagate_work_source(&mut self) {
        if let Some(mut state) = self.transaction {
            state.propagate_work_source = false;
        }
    }

    fn clear_calling_work_source(&mut self) {
        self.set_calling_work_source_uid(UNSET_WORK_SOURCE as _);
    }

    fn set_calling_work_source_uid(&mut self, uid: binder::uid_t) -> i64 {
        let token = self.set_calling_work_source_uid_without_propagation(uid);
        if let Some(mut state) = self.transaction {
            state.propagate_work_source = true;
        }
        token
    }

    pub(crate) fn set_calling_work_source_uid_without_propagation(&mut self, uid: binder::uid_t) -> i64 {
        match self.transaction {
            Some(mut state) => {
                let propagated_bit = (state.propagate_work_source as i64) << WORK_SOURCE_PROPAGATED_BIT_INDEX;
                let token = propagated_bit | (state.work_source as i64);
                state.work_source = uid;

                token
            }
            None => {
                0
            }
        }
    }

// status_t IPCThreadState::writeTransactionData(int32_t cmd, uint32_t binderFlags,
//     int32_t handle, uint32_t code, const Parcel& data, status_t* statusBuffer)
// {
//     binder_transaction_data tr;

//     tr.target.ptr = 0; /* Don't pass uninitialized stack data to a remote process */
//     tr.target.handle = handle;
//     tr.code = code;
//     tr.flags = binderFlags;
//     tr.cookie = 0;
//     tr.sender_pid = 0;
//     tr.sender_euid = 0;

//     const status_t err = data.errorCheck();
//     if (err == NO_ERROR) {
//         tr.data_size = data.ipcDataSize();
//         tr.data.ptr.buffer = data.ipcData();
//         tr.offsets_size = data.ipcObjectsCount()*sizeof(binder_size_t);
//         tr.data.ptr.offsets = data.ipcObjects();
//     } else if (statusBuffer) {
//         tr.flags |= TF_STATUS_CODE;
//         *statusBuffer = err;
//         tr.data_size = sizeof(status_t);
//         tr.data.ptr.buffer = reinterpret_cast<uintptr_t>(statusBuffer);
//         tr.offsets_size = 0;
//         tr.data.ptr.offsets = 0;
//     } else {
//         return (mLastError = err);
//     }

//     mOut.writeInt32(cmd);
//     mOut.write(&tr, sizeof(tr));

//     return NO_ERROR;
// }

    fn write_transaction_data(&mut self, cmd: u32, flags: u32, handle: u32, code: u32, data: &Parcel) -> Result<()> {
        // ptr is initialized by zero because ptr(64) and handle(32) size is different.
        let mut target = binder_transaction_data__bindgen_ty_1 {
            ptr: 0,
        };
        target.handle = handle;

        let tr = binder_transaction_data {
            target,
            cookie: 0 ,
            code,
            flags,
            sender_pid: 0,
            sender_euid: 0,
            data_size: data.len() as _,
            offsets_size: (data.objects.len() * std::mem::size_of::<binder_size_t>()) as _,
            data: binder_transaction_data__bindgen_ty_2 {
                ptr: binder_transaction_data__bindgen_ty_2__bindgen_ty_1 {
                    buffer: data.as_ptr() as _,
                    offsets: data.objects.as_ptr() as _,
                },
                // buf: ,  // [__u8; 8usize],
            }
        };

        self.out_parcel.write::<u32>(&cmd)?;
        unsafe {
            let ptr = &tr as *const binder_transaction_data;
            self.out_parcel.write_data(
                std::slice::from_raw_parts(ptr as _, std::mem::size_of::<binder_transaction_data>())
            );
        }

        Ok(())
    }
}

pub(crate) fn set_call_restriction(call_restriction: CallRestriction) {
    THREAD_STATE.with(|thread_state| {
        thread_state.borrow_mut().call_restriction = call_restriction;
    })
}

pub(crate) fn call_restriction() -> CallRestriction {
    THREAD_STATE.with(|thread_state| {
        thread_state.borrow().call_restriction
    })
}

pub(crate) fn strict_mode_policy() -> i32 {
    THREAD_STATE.with(|thread_state| {
        thread_state.borrow().strict_mode_policy
    })
}

pub(crate) fn should_propagate_work_source() -> bool {
    THREAD_STATE.with(|thread_state| {
        thread_state.borrow().transaction.map_or(false, |state| state.propagate_work_source)
    })
}

pub(crate) fn calling_work_source_uid() -> binder::uid_t {
    THREAD_STATE.with(|thread_state| {
        thread_state.borrow().transaction.map_or(0, |state| state.work_source)
    })
}

pub fn setup_polling() -> Result<std::os::unix::io::RawFd> {
    THREAD_STATE.with(|thread_state| -> Result<()> {
        thread_state.borrow_mut().out_parcel.write::<u32>(&binder::BC_ENTER_LOOPER)
    })?;
    flash_commands()?;
    Ok(ProcessState::as_self().as_raw_fd())
}

enum UntilResponse {
    Reply,
    TransactionComplete,
    AcquireResult(StatusCode),
}

fn wait_for_response(until: &mut UntilResponse) -> Result<Option<Parcel>> {
    THREAD_STATE.with(|thread_state| -> Result<Option<Parcel>> {
        loop {
            talk_with_driver(true)?;

            if thread_state.borrow().in_parcel.is_empty()  {
                continue;
            }
            let cmd: u32 = thread_state.borrow_mut().in_parcel.read::<i32>()? as _;

            log::trace!("{:?}", return_to_str(cmd));

            match cmd {
                binder::BR_ONEWAY_SPAM_SUSPECT => {
                    todo!("wait_for_response - BR_ONEWAY_SPAM_SUSPECT");
                },
                binder::BR_TRANSACTION_COMPLETE => {
                    if let UntilResponse::TransactionComplete = until {
                        break
                    }
                }
                binder::BR_DEAD_REPLY => {
                    todo!("wait_for_response - BR_DEAD_REPLY");
                },
                binder::BR_FAILED_REPLY => {
                    todo!("wait_for_response - BR_FAILED_REPLY");
                },
                binder::BR_FROZEN_REPLY => {
                    todo!("wait_for_response - BR_FROZEN_REPLY");
                },
                binder::BR_ACQUIRE_RESULT => {
                    let result = thread_state.borrow_mut().in_parcel.read::<i32>()?;
                    if let UntilResponse::AcquireResult(exception) = until {
                        *exception = if result != 0 {
                            StatusCode::Ok
                        } else {
                            StatusCode::InvalidOperation
                        };
                        break
                    }
                },
                binder::BR_REPLY => {
                    let tr = thread_state.borrow_mut().in_parcel.read::<binder::binder_transaction_data>()?;

                    if let UntilResponse::Reply = until {
                        if (tr.flags & transaction_flags_TF_STATUS_CODE) == 0 {
                            let reply = unsafe {
                                Parcel::from_ipc_parts(tr.data.ptr.buffer as _, tr.data_size as _,
                                    tr.data.ptr.offsets as _,
                                    (tr.offsets_size as usize) / std::mem::size_of::<binder::binder_size_t>(),
                                    free_buffer)
                            };

                            return Ok(Some(reply));
                        } else {
                            unsafe {
                                let status = *(tr.data.ptr.buffer as *const i32);
                                free_buffer(None,
                                    tr.data.ptr.buffer,
                                    tr.data_size as _,
                                    tr.data.ptr.offsets,
                                    (tr.offsets_size as usize) / std::mem::size_of::<binder_size_t>())?;

                                if status != StatusCode::Ok as _ {
                                    return Err(Status::from_i32_status(status,
                                        ExceptionCode::None, "binder::BR_REPLY").into())
                                }
                            };
                        }
                    } else {
                        unsafe {
                            free_buffer(None,
                                tr.data.ptr.buffer,
                                tr.data_size as _,
                                tr.data.ptr.offsets,
                                (tr.offsets_size as usize) / std::mem::size_of::<binder_size_t>())?;
                        }
                    }
                },
                _ => execute_command(cmd as _)?,
            };
        };
        Ok(None)
    })
}

fn execute_command(cmd: i32) -> Result<()> {
    let cmd: std::os::raw::c_uint = cmd as _;

    THREAD_STATE.with(|thread_state| -> Result<()> {
        match cmd {
            binder::BR_ERROR => {
                let other = thread_state.borrow_mut().in_parcel.read::<i32>()?;
                return Err(Status::from_i32_status(other, ExceptionCode::JustError, "binder::BR_ERROR").into());
            }
            binder::BR_OK => {}

            binder::BR_TRANSACTION_SEC_CTX |
            binder::BR_TRANSACTION => {
                let tr_secctx = {
                    let mut thread_state = thread_state.borrow_mut();
                    if cmd == binder::BR_TRANSACTION_SEC_CTX {
                        thread_state.in_parcel.read::<binder::binder_transaction_data_secctx>()?
                    } else {
                        binder::binder_transaction_data_secctx {
                            transaction_data: thread_state.in_parcel.read::<binder::binder_transaction_data>()?,
                            secctx: 0,
                        }
                    }
                };

                let mut reader = unsafe {
                    let tr = &tr_secctx.transaction_data;

                    Parcel::from_ipc_parts(tr.data.ptr.buffer as _, tr.data_size as _,
                        tr.data.ptr.offsets as _, (tr.offsets_size as usize) / std::mem::size_of::<binder::binder_size_t>(),
                        free_buffer)
                };

                // TODO: Skip now, because if below implmentation is mandatory.
                // const void* origServingStackPointer = mServingStackPointer;
                // mServingStackPointer = &origServingStackPointer; // anything on the stack

                let transaction_old = {
                    let mut thread_state = thread_state.borrow_mut();
                    let transaction_old = thread_state.transaction;

                    thread_state.clear_calling_work_source();
                    thread_state.clear_propagate_work_source();

                    thread_state.transaction = Some(TransactionState::from_transaction_data(&tr_secctx));

                    transaction_old
                };

                let mut reply = Parcel::new();

                unsafe {
                    let target_ptr = tr_secctx.transaction_data.target.ptr;
                    if target_ptr != 0 {
                        todo!("need to call BBinder->transact()")
                        // let weak_ref: *mut ref_base::WeakRef = target_ptr as _;
                        // let mut ref_base = ref_base::RefBase::<ref_base::RemoteRef>::from_raw(target_ptr as _);
                        // if ref_base.attempt_inc_strong() {
                        //     todo!("need to call BBinder->transact()")
                        // }
                //     if (reinterpret_cast<RefBase::weakref_type*>(
                //             tr.target.ptr)->attemptIncStrong(this)) {
                //         error = reinterpret_cast<BBinder*>(tr.cookie)->transact(tr.code, buffer,
                //                 &reply, tr.flags);
                //         reinterpret_cast<BBinder*>(tr.cookie)->decStrong(this);
                //     } else {
                //         error = UNKNOWN_TRANSACTION;
                //     }

                    } else {
                        let context = ProcessState::as_self().context_manager();
                        if let Some(context) = context {
                            reader.set_data_position(0);
                            context.transact(tr_secctx.transaction_data.code, &mut reader, &mut reply)?;
                        }
                    }
                }
                let flags = tr_secctx.transaction_data.flags;
                if (flags & transaction_flags_TF_ONE_WAY) == 0 {
                    let flags = flags & transaction_flags_TF_CLEAR_BUF;
                    thread_state.borrow_mut().write_transaction_data(binder::BC_REPLY, flags, u32::MAX, 0, &reply)?;
                    reply.set_len(0);
                    wait_for_response(&mut UntilResponse::TransactionComplete)?;
                }

                thread_state.borrow_mut().transaction = transaction_old;
            }

            binder::BR_REPLY => {
                todo!("execute_command - BR_REPLY");
            }
            binder::BR_ACQUIRE_RESULT => {
                todo!("execute_command - BR_ACQUIRE_RESULT");
            }
            binder::BR_DEAD_REPLY => {
                todo!("execute_command - BR_DEAD_REPLY");
            }
            binder::BR_TRANSACTION_COMPLETE => {
                todo!("execute_command - BR_TRANSACTION_COMPLETE");
            }
            binder::BR_INCREFS => {
                todo!("execute_command - BR_INCREFS");
        // refs = (RefBase::weakref_type*)mIn.readPointer();
        // obj = (BBinder*)mIn.readPointer();
        // refs->incWeak(mProcess.get());
        // mOut.writeInt32(BC_INCREFS_DONE);
        // mOut.writePointer((uintptr_t)refs);
        // mOut.writePointer((uintptr_t)obj);

            }
            binder::BR_ACQUIRE => {
                let mut state = thread_state.borrow_mut();
                let raw_pointer = {
                    state.in_parcel.read::<*const dyn IBinder>()?
                };

                let strong: Arc<dyn IBinder> = unsafe { Arc::from_raw(raw_pointer) };

                {
                    state.out_parcel.write::<i32>(&(binder::BC_ACQUIRE_DONE as i32))?;
                    state.out_parcel.write::<*const dyn IBinder>(&Arc::into_raw(strong))?;
                }
            }
            binder::BR_RELEASE => {
                todo!("execute_command - BR_RELEASE");
        // refs = (RefBase::weakref_type*)mIn.readPointer();
        // obj = (BBinder*)mIn.readPointer();
        // ALOG_ASSERT(refs->refBase() == obj,
        //            "BR_RELEASE: object %p does not match cookie %p (expected %p)",
        //            refs, obj, refs->refBase());
        // IF_LOG_REMOTEREFS() {
        //     LOG_REMOTEREFS("BR_RELEASE from driver on %p", obj);
        //     obj->printRefs();
        // }
        // mPendingStrongDerefs.push(obj);

            }
            binder::BR_DECREFS => {
                todo!("execute_command - BR_DECREFS");
        // refs = (RefBase::weakref_type*)mIn.readPointer();
        // obj = (BBinder*)mIn.readPointer();
        // // NOTE: This assertion is not valid, because the object may no
        // // longer exist (thus the (BBinder*)cast above resulting in a different
        // // memory address).
        // //ALOG_ASSERT(refs->refBase() == obj,
        // //           "BR_DECREFS: object %p does not match cookie %p (expected %p)",
        // //           refs, obj, refs->refBase());
        // mPendingWeakDerefs.push(refs);
            }
            binder::BR_ATTEMPT_ACQUIRE => {
                todo!("execute_command - BR_ATTEMPT_ACQUIRE");
        // refs = (RefBase::weakref_type*)mIn.readPointer();
        // obj = (BBinder*)mIn.readPointer();

        // {
        //     const bool success = refs->attemptIncStrong(mProcess.get());
        //     ALOG_ASSERT(success && refs->refBase() == obj,
        //                "BR_ATTEMPT_ACQUIRE: object %p does not match cookie %p (expected %p)",
        //                refs, obj, refs->refBase());

        //     mOut.writeInt32(BC_ACQUIRE_RESULT);
        //     mOut.writeInt32((int32_t)success);
        // }
            }
            binder::BR_NOOP => {}
            binder::BR_SPAWN_LOOPER => {
                todo!("execute_command - BR_SPAWN_LOOPER");
            }
            binder::BR_FINISHED => {
                todo!("execute_command - BR_FINISHED");
            }
            binder::BR_DEAD_BINDER => {
                todo!("Bexecute_command - R_DEAD_BINDER");
            }
            binder::BR_CLEAR_DEATH_NOTIFICATION_DONE => {
                todo!("execute_command - BR_CLEAR_DEATH_NOTIFICATION_DONE");
            }
            binder::BR_FAILED_REPLY => {
                todo!("execute_command - BR_FAILED_REPLY");
            }
            binder::BR_FROZEN_REPLY => {
                todo!("execute_command - BR_FROZEN_REPLY");
            }
            binder::BR_ONEWAY_SPAM_SUSPECT => {
                todo!("execute_command - BR_ONEWAY_SPAM_SUSPECT");
            }
            _ => {}
        };

        Ok(())
    })
}


fn talk_with_driver(do_receive: bool) -> Result<()> {
    let driver_fd = ProcessState::as_self().as_raw_fd();
    if driver_fd < 0 {
        return Err(Error::from(StatusCode::BadFd));
    }

    THREAD_STATE.with(|thread_state| -> Result<()> {
        let mut bwr = {
            let mut thread_state = thread_state.borrow_mut();
            let need_read = thread_state.in_parcel.is_empty();
            let out_avail = if !do_receive || need_read {
                thread_state.out_parcel.len()
            } else {
                0
            };

            let read_size = if do_receive && need_read {
                thread_state.in_parcel.capacity()
            } else {
                0
            };
            binder::binder_write_read {
                write_size: out_avail as _,
                write_consumed: 0,
                write_buffer: thread_state.out_parcel.as_mut_ptr() as _,
                read_size: read_size as _,
                read_consumed: 0,
                read_buffer: thread_state.in_parcel.as_mut_ptr() as _,
            }
        };

        if bwr.write_size == 0 && bwr.read_size == 0 {
            return Ok(())
        }

        if bwr.write_size != 0 {
            log::trace!("Sending command to driver:\n{:?}", thread_state.borrow().out_parcel);
            log::trace!("Size of receive buffer: {}, need_read: {}, do_receive: {}",
                bwr.read_size, thread_state.borrow().in_parcel.is_empty(), do_receive);
        }

        unsafe {
            loop {
                let res = binder::write_read(driver_fd, &mut bwr);
                match res {
                    Ok(_) => break,
                    Err(errno) if errno != nix::errno::Errno::EINTR => {
                        return Err(Error::Any(errno.into()));
                    },
                    _ => {}
                }

            }
        }

        {
            let mut thread_state = thread_state.borrow_mut();

            if bwr.write_consumed > 0 {
                if bwr.write_consumed < thread_state.out_parcel.len() as _ {
                    panic!("Driver did not consume write buffer. consumed: {} of {}",
                        bwr.write_consumed, thread_state.out_parcel.len());
                } else {
                    thread_state.out_parcel.set_len(0);
                    thread_state.process_post_write_derefs();
                }
            }

            if bwr.read_consumed > 0 {
                thread_state.in_parcel.set_len(bwr.read_consumed as _);
                thread_state.in_parcel.set_data_position(0);

                log::trace!("Received commands to driver:\n{:?}", thread_state.in_parcel);
            }
        };

        Ok(())
    })
}

fn get_and_execute_command() -> Result<()> {
    talk_with_driver(true)?;

    let cmd = THREAD_STATE.with(|thread_state| -> Result<i32> {
        thread_state.borrow_mut().in_parcel.read::<i32>()
    })?;
    execute_command(cmd)?;

    Ok(())
}

fn flash_commands() -> Result<()> {
    talk_with_driver(false)?;

    THREAD_STATE.with(|thread_state| -> Result<()> {
        let out_len = thread_state.borrow().out_parcel.len();
        if out_len > 0 {
            talk_with_driver(false)?;
        }

        let out_len = thread_state.borrow().out_parcel.len();
        if out_len > 0 {
            log::warn!("self.out_parcel.len() > 0 after flash_commands()");
        }

        Ok(())
    })
}


pub fn inc_strong_handle(handle: u32, proxy: StrongIBinder) -> Result<()> {
    THREAD_STATE.with(|thread_state| -> Result<()> {
        {
            let mut state = thread_state.borrow_mut();

            state.out_parcel.write::<u32>(&(binder::BC_ACQUIRE))?;
            state.out_parcel.write::<u32>(&(handle))?;
        }

        if !(flash_if_needed()?) {
            thread_state.borrow_mut().post_strong_derefs.push(proxy);
        }

        Ok(())
    })
}

pub fn dec_strong_handle(handle: u32) -> Result<()> {
    THREAD_STATE.with(|thread_state| -> Result<()> {
        {
            let mut state = thread_state.borrow_mut();

            state.out_parcel.write::<u32>(&(binder::BC_RELEASE))?;
            state.out_parcel.write::<u32>(&(handle))?;
        }

        flash_if_needed()?;

        Ok(())
    })
}

pub fn inc_weak_handle(handle: u32, weak: WeakIBinder) -> Result<()>{
    THREAD_STATE.with(|thread_state| -> Result<()> {
        {
            let mut state = thread_state.borrow_mut();

            state.out_parcel.write::<u32>(&(binder::BC_INCREFS))?;
            state.out_parcel.write::<u32>(&(handle))?;
        }

        if !(flash_if_needed()?) {
            // This code is come from IPCThreadState.cpp. Is it necessaryq?
            thread_state.borrow_mut().post_weak_derefs.push(weak);
        }

        Ok(())
    })
}

pub fn dec_weak_handle(handle: u32) -> Result<()> {
    THREAD_STATE.with(|thread_state| -> Result<()> {
        {
            let mut state = thread_state.borrow_mut();

            state.out_parcel.write::<u32>(&(binder::BC_DECREFS))?;
            state.out_parcel.write::<u32>(&(handle))?;
        }

        flash_if_needed()?;

        Ok(())
    })
}


pub fn flash_if_needed() -> Result<bool> {
    THREAD_STATE.with(|thread_state| -> Result<bool> {
        {
            let thread_state = thread_state.borrow();
            if thread_state.is_looper || thread_state.is_flushing {
                return Ok(false);
            }
        }

        thread_state.borrow_mut().is_flushing = true;
        flash_commands()?;
        thread_state.borrow_mut().is_flushing = false;

        Ok(true)
    })
}

pub fn handle_commands() -> Result<()> {
    while {
        get_and_execute_command()?;

        THREAD_STATE.with(|thread_state| -> bool {
            !thread_state.borrow().in_parcel.is_empty()
        })
    } {
        flash_commands()?;
    }
    Ok(())
}

pub fn check_interface<T: Remotable>(reader: &mut Parcel) -> Result<()> {
    let mut strict_policy: i32 = reader.read()?;

    THREAD_STATE.with(|thread_state| -> Result<()> {
        let mut thread_state = thread_state.borrow_mut();

        if (thread_state.last_transaction_binder_flags() & FLAG_ONEWAY) != 0 {
            strict_policy = 0;
        }
        thread_state.set_strict_mode_policy(strict_policy);
        reader.update_work_source_request_header_pos();

        let work_source: i32 = reader.read()?;
        thread_state.set_calling_work_source_uid_without_propagation(work_source as _);

        let header: u32 = reader.read()?;
        if header != INTERFACE_HEADER {
            return Err(Status::new(StatusCode::Ok, ExceptionCode::BadParcelable,
                &format!("Expecting header {:#x} but found {:#x}.", INTERFACE_HEADER, header)).into());
        }

        Ok(())
    })?;

    let parcel_interface: String16 = reader.read()?;
    if parcel_interface.0.eq(T::get_descriptor()) {
        Ok(())
    } else {
        Err(Status::new(StatusCode::Ok, ExceptionCode::BadParcelable,
            &format!("check_interface() expected '{}' but read '{}'",
                T::get_descriptor(), parcel_interface.0)).into())
    }
}

pub fn transact(handle: u32, code: u32, data: &Parcel, mut flags: u32) -> Result<Option<Parcel>> {
    let mut reply: Option<Parcel> = None;

    flags |= transaction_flags_TF_ACCEPT_FDS;

    let call_restriction = THREAD_STATE.with(|thread_state| -> Result<CallRestriction> {
        let mut thread_state = thread_state.borrow_mut();
        thread_state.write_transaction_data(binder::BC_TRANSACTION, flags, handle, code, data)?;
        Ok(thread_state.call_restriction)
    })?;

    if (flags & transaction_flags_TF_ONE_WAY) == 0 {
        match call_restriction {
            CallRestriction::ErrorIfNotOneway => {
                error!("Process making non-oneway call (code: {}) but is restricted.", code)
            },
            CallRestriction::FatalIfNotOneway => {
                panic!("Process may not make non-oneway calls (code: {}).", code);
            },
            _ => (),
        }

        reply = wait_for_response(&mut UntilResponse::Reply)?
    } else {
        wait_for_response(&mut UntilResponse::TransactionComplete)?;
    }

    Ok(reply)
}


fn free_buffer(parcel: Option<&Parcel>, data: binder_uintptr_t, _: usize, _ : binder_uintptr_t, _: usize) -> Result<()> {
    if let Some(parcel) = parcel {
        parcel.close_file_descriptors()
    }

    THREAD_STATE.with(|thread_state| -> Result<()> {
        let mut thread_state = thread_state.borrow_mut();
        thread_state.out_parcel.write::<u32>(&binder::BC_FREE_BUFFER)?;
        thread_state.out_parcel.write::<binder_uintptr_t>(&data)?;
        Ok(())
    })?;

    flash_if_needed()?;

    Ok(())
}

pub(crate) fn query_interface(handle: u32) -> Result<String> {
    let data = Parcel::new();
    let reply = transact(handle, INTERFACE_TRANSACTION, &data, 0)?;
    let interface: String16 = reply.expect("INTERFACE_TRANSACTION should have reply parcel").read()?;

    Ok(interface.0)
}

pub(crate) fn ping_binder(handle: u32) -> Result<()> {
    let data = Parcel::new();
    let _reply = transact(handle, PING_TRANSACTION, &data, 0)?;
    Ok(())
}


// void freeBuffer(Parcel* parcel, const uint8_t* data,
//                                 size_t /*dataSize*/,
//                                 const binder_size_t* /*objects*/,
//                                 size_t /*objectsSize*/)
// {
//     //ALOGI("Freeing parcel %p", &parcel);
//     IF_LOG_COMMANDS() {
//         alog << "Writing BC_FREE_BUFFER for " << data << endl;
//     }
//     ALOG_ASSERT(data != NULL, "Called with NULL data");
//     if (parcel != nullptr) parcel->closeFileDescriptors();
//     IPCThreadState* state = self();
//     state->mOut.writeInt32(BC_FREE_BUFFER);
//     state->mOut.writePointer((uintptr_t)data);
    // state->flushIfNeeded();
// }

// status_t IPCThreadState::setupPolling(int* fd)
// {
//     if (mProcess->mDriverFD < 0) {
//         return -EBADF;
//     }

//     mOut.writeInt32(BC_ENTER_LOOPER);
//     flushCommands();
//     *fd = mProcess->mDriverFD;
//     return 0;
// }




// void IPCThreadState::flushCommands()
// {
//     if (mProcess->mDriverFD < 0)
//         return;
//     talkWithDriver(false);
//     // The flush could have caused post-write refcount decrements to have
//     // been executed, which in turn could result in BC_RELEASE/BC_DECREFS
//     // being queued in mOut. So flush again, if we need to.
//     if (mOut.dataSize() > 0) {
//         talkWithDriver(false);
//     }
//     if (mOut.dataSize() > 0) {
//         ALOGW("mOut.dataSize() > 0 after flushCommands()");
//     }
// }


// status_t IPCThreadState::executeCommand(int32_t cmd)
// {
//     BBinder* obj;
//     RefBase::weakref_type* refs;
//     status_t result = NO_ERROR;

//     switch ((uint32_t)cmd) {
//     case BR_ERROR:
//         result = mIn.readInt32();
//         break;

//     case BR_OK:
//         break;

//     case BR_ACQUIRE:
//         refs = (RefBase::weakref_type*)mIn.readPointer();
//         obj = (BBinder*)mIn.readPointer();
//         ALOG_ASSERT(refs->refBase() == obj,
//                    "BR_ACQUIRE: object %p does not match cookie %p (expected %p)",
//                    refs, obj, refs->refBase());
//         obj->incStrong(mProcess.get());
//         IF_LOG_REMOTEREFS() {
//             LOG_REMOTEREFS("BR_ACQUIRE from driver on %p", obj);
//             obj->printRefs();
//         }
//         mOut.writeInt32(BC_ACQUIRE_DONE);
//         mOut.writePointer((uintptr_t)refs);
//         mOut.writePointer((uintptr_t)obj);
//         break;

//     case BR_RELEASE:
//         refs = (RefBase::weakref_type*)mIn.readPointer();
//         obj = (BBinder*)mIn.readPointer();
//         ALOG_ASSERT(refs->refBase() == obj,
//                    "BR_RELEASE: object %p does not match cookie %p (expected %p)",
//                    refs, obj, refs->refBase());
//         IF_LOG_REMOTEREFS() {
//             LOG_REMOTEREFS("BR_RELEASE from driver on %p", obj);
//             obj->printRefs();
//         }
//         mPendingStrongDerefs.push(obj);
//         break;

//     case BR_INCREFS:
//         refs = (RefBase::weakref_type*)mIn.readPointer();
//         obj = (BBinder*)mIn.readPointer();
//         refs->incWeak(mProcess.get());
//         mOut.writeInt32(BC_INCREFS_DONE);
//         mOut.writePointer((uintptr_t)refs);
//         mOut.writePointer((uintptr_t)obj);
//         break;

//     case BR_DECREFS:
//         refs = (RefBase::weakref_type*)mIn.readPointer();
//         obj = (BBinder*)mIn.readPointer();
//         // NOTE: This assertion is not valid, because the object may no
//         // longer exist (thus the (BBinder*)cast above resulting in a different
//         // memory address).
//         //ALOG_ASSERT(refs->refBase() == obj,
//         //           "BR_DECREFS: object %p does not match cookie %p (expected %p)",
//         //           refs, obj, refs->refBase());
//         mPendingWeakDerefs.push(refs);
//         break;

//     case BR_ATTEMPT_ACQUIRE:
//         refs = (RefBase::weakref_type*)mIn.readPointer();
//         obj = (BBinder*)mIn.readPointer();

//         {
//             const bool success = refs->attemptIncStrong(mProcess.get());
//             ALOG_ASSERT(success && refs->refBase() == obj,
//                        "BR_ATTEMPT_ACQUIRE: object %p does not match cookie %p (expected %p)",
//                        refs, obj, refs->refBase());

//             mOut.writeInt32(BC_ACQUIRE_RESULT);
//             mOut.writeInt32((int32_t)success);
//         }
//         break;

//     case BR_TRANSACTION_SEC_CTX:
//     case BR_TRANSACTION:
//         {
//             binder_transaction_data_secctx tr_secctx;
//             binder_transaction_data& tr = tr_secctx.transaction_data;

//             if (cmd == (int) BR_TRANSACTION_SEC_CTX) {
//                 result = mIn.read(&tr_secctx, sizeof(tr_secctx));
//             } else {
//                 result = mIn.read(&tr, sizeof(tr));
//                 tr_secctx.secctx = 0;
//             }

//             ALOG_ASSERT(result == NO_ERROR,
//                 "Not enough command data for brTRANSACTION");
//             if (result != NO_ERROR) break;

//             Parcel buffer;
//             buffer.ipcSetDataReference(
//                 reinterpret_cast<const uint8_t*>(tr.data.ptr.buffer),
//                 tr.data_size,
//                 reinterpret_cast<const binder_size_t*>(tr.data.ptr.offsets),
//                 tr.offsets_size/sizeof(binder_size_t), freeBuffer);

//             const void* origServingStackPointer = mServingStackPointer;
//             mServingStackPointer = &origServingStackPointer; // anything on the stack

//             const pid_t origPid = mCallingPid;
//             const char* origSid = mCallingSid;
//             const uid_t origUid = mCallingUid;
//             const int32_t origStrictModePolicy = mStrictModePolicy;
//             const int32_t origTransactionBinderFlags = mLastTransactionBinderFlags;
//             const int32_t origWorkSource = mWorkSource;
//             const bool origPropagateWorkSet = mPropagateWorkSource;
//             // Calling work source will be set by Parcel#enforceInterface. Parcel#enforceInterface
//             // is only guaranteed to be called for AIDL-generated stubs so we reset the work source
//             // here to never propagate it.
//             clearCallingWorkSource();
//             clearPropagateWorkSource();

//             mCallingPid = tr.sender_pid;
//             mCallingSid = reinterpret_cast<const char*>(tr_secctx.secctx);
//             mCallingUid = tr.sender_euid;
//             mLastTransactionBinderFlags = tr.flags;

//             // ALOGI(">>>> TRANSACT from pid %d sid %s uid %d\n", mCallingPid,
//             //    (mCallingSid ? mCallingSid : "<N/A>"), mCallingUid);

//             Parcel reply;
//             status_t error;
//             IF_LOG_TRANSACTIONS() {
//                 TextOutput::Bundle _b(alog);
//                 alog << "BR_TRANSACTION thr " << (void*)pthread_self()
//                     << " / obj " << tr.target.ptr << " / code "
//                     << TypeCode(tr.code) << ": " << indent << buffer
//                     << dedent << endl
//                     << "Data addr = "
//                     << reinterpret_cast<const uint8_t*>(tr.data.ptr.buffer)
//                     << ", offsets addr="
//                     << reinterpret_cast<const size_t*>(tr.data.ptr.offsets) << endl;
//             }
//             if (tr.target.ptr) {
//                 // We only have a weak reference on the target object, so we must first try to
//                 // safely acquire a strong reference before doing anything else with it.
//                 if (reinterpret_cast<RefBase::weakref_type*>(
//                         tr.target.ptr)->attemptIncStrong(this)) {
//                     error = reinterpret_cast<BBinder*>(tr.cookie)->transact(tr.code, buffer,
//                             &reply, tr.flags);
//                     reinterpret_cast<BBinder*>(tr.cookie)->decStrong(this);
//                 } else {
//                     error = UNKNOWN_TRANSACTION;
//                 }

//             } else {
//                 error = the_context_object->transact(tr.code, buffer, &reply, tr.flags);
//             }

//             //ALOGI("<<<< TRANSACT from pid %d restore pid %d sid %s uid %d\n",
//             //     mCallingPid, origPid, (origSid ? origSid : "<N/A>"), origUid);

//             if ((tr.flags & TF_ONE_WAY) == 0) {
//                 LOG_ONEWAY("Sending reply to %d!", mCallingPid);
//                 if (error < NO_ERROR) reply.setError(error);

//                 constexpr uint32_t kForwardReplyFlags = TF_CLEAR_BUF;
//                 sendReply(reply, (tr.flags & kForwardReplyFlags));
//             } else {
//                 if (error != OK) {
//                     alog << "oneway function results for code " << tr.code
//                          << " on binder at "
//                          << reinterpret_cast<void*>(tr.target.ptr)
//                          << " will be dropped but finished with status "
//                          << statusToString(error);

//                     // ideally we could log this even when error == OK, but it
//                     // causes too much logspam because some manually-written
//                     // interfaces have clients that call methods which always
//                     // write results, sometimes as oneway methods.
//                     if (reply.dataSize() != 0) {
//                          alog << " and reply parcel size " << reply.dataSize();
//                     }

//                     alog << endl;
//                 }
//                 LOG_ONEWAY("NOT sending reply to %d!", mCallingPid);
//             }

//             mServingStackPointer = origServingStackPointer;
//             mCallingPid = origPid;
//             mCallingSid = origSid;
//             mCallingUid = origUid;
//             mStrictModePolicy = origStrictModePolicy;
//             mLastTransactionBinderFlags = origTransactionBinderFlags;
//             mWorkSource = origWorkSource;
//             mPropagateWorkSource = origPropagateWorkSet;

//             IF_LOG_TRANSACTIONS() {
//                 TextOutput::Bundle _b(alog);
//                 alog << "BC_REPLY thr " << (void*)pthread_self() << " / obj "
//                     << tr.target.ptr << ": " << indent << reply << dedent << endl;
//             }

//         }
//         break;

//     case BR_DEAD_BINDER:
//         {
//             BpBinder *proxy = (BpBinder*)mIn.readPointer();
//             proxy->sendObituary();
//             mOut.writeInt32(BC_DEAD_BINDER_DONE);
//             mOut.writePointer((uintptr_t)proxy);
//         } break;

//     case BR_CLEAR_DEATH_NOTIFICATION_DONE:
//         {
//             BpBinder *proxy = (BpBinder*)mIn.readPointer();
//             proxy->getWeakRefs()->decWeak(proxy);
//         } break;

//     case BR_FINISHED:
//         result = TIMED_OUT;
//         break;

//     case BR_NOOP:
//         break;

//     case BR_SPAWN_LOOPER:
//         mProcess->spawnPooledThread(false);
//         break;

//     default:
//         ALOGE("*** BAD COMMAND %d received from Binder driver\n", cmd);
//         result = UNKNOWN_ERROR;
//         break;
//     }

//     if (result != NO_ERROR) {
//         mLastError = result;
//     }

//     return result;
// }


// status_t IPCThreadState::waitForResponse(Parcel *reply, status_t *acquireResult)
// {
//     uint32_t cmd;
//     int32_t err;

//     while (1) {
//         if ((err=talkWithDriver()) < NO_ERROR) break;
//         err = mIn.errorCheck();
//         if (err < NO_ERROR) break;
//         if (mIn.dataAvail() == 0) continue;

//         cmd = (uint32_t)mIn.readInt32();

//         IF_LOG_COMMANDS() {
//             alog << "Processing waitForResponse Command: "
//                 << getReturnString(cmd) << endl;
//         }

//         switch (cmd) {
//         case BR_ONEWAY_SPAM_SUSPECT:
//             ALOGE("Process seems to be sending too many oneway calls.");
//             CallStack::logStack("oneway spamming", CallStack::getCurrent().get(),
//                     ANDROID_LOG_ERROR);
//             [[fallthrough]];
//         case BR_TRANSACTION_COMPLETE:
//             if (!reply && !acquireResult) goto finish;
//             break;

//         case BR_DEAD_REPLY:
//             err = DEAD_OBJECT;
//             goto finish;

//         case BR_FAILED_REPLY:
//             err = FAILED_TRANSACTION;
//             goto finish;

//         case BR_FROZEN_REPLY:
//             err = FAILED_TRANSACTION;
//             goto finish;

//         case BR_ACQUIRE_RESULT:
//             {
//                 ALOG_ASSERT(acquireResult != NULL, "Unexpected brACQUIRE_RESULT");
//                 const int32_t result = mIn.readInt32();
//                 if (!acquireResult) continue;
//                 *acquireResult = result ? NO_ERROR : INVALID_OPERATION;
//             }
//             goto finish;

//         case BR_REPLY:
//             {
//                 binder_transaction_data tr;
//                 err = mIn.read(&tr, sizeof(tr));
//                 ALOG_ASSERT(err == NO_ERROR, "Not enough command data for brREPLY");
//                 if (err != NO_ERROR) goto finish;

//                 if (reply) {
//                     if ((tr.flags & TF_STATUS_CODE) == 0) {
//                         reply->ipcSetDataReference(
//                             reinterpret_cast<const uint8_t*>(tr.data.ptr.buffer),
//                             tr.data_size,
//                             reinterpret_cast<const binder_size_t*>(tr.data.ptr.offsets),
//                             tr.offsets_size/sizeof(binder_size_t),
//                             freeBuffer);
//                     } else {
//                         err = *reinterpret_cast<const status_t*>(tr.data.ptr.buffer);
//                         freeBuffer(nullptr,
//                             reinterpret_cast<const uint8_t*>(tr.data.ptr.buffer),
//                             tr.data_size,
//                             reinterpret_cast<const binder_size_t*>(tr.data.ptr.offsets),
//                             tr.offsets_size/sizeof(binder_size_t));
//                     }
//                 } else {
//                     freeBuffer(nullptr,
//                         reinterpret_cast<const uint8_t*>(tr.data.ptr.buffer),
//                         tr.data_size,
//                         reinterpret_cast<const binder_size_t*>(tr.data.ptr.offsets),
//                         tr.offsets_size/sizeof(binder_size_t));
//                     continue;
//                 }
//             }
//             goto finish;

//         default:
//             err = executeCommand(cmd);
//             if (err != NO_ERROR) goto finish;
//             break;
//         }
//     }

// finish:
//     if (err != NO_ERROR) {
//         if (acquireResult) *acquireResult = err;
//         if (reply) reply->setError(err);
//         mLastError = err;
//     }

//     return err;
// }


