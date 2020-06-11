use crate::{Identifier, Key};
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};

static mut RECORDER: &'static dyn Recorder = &NoopRecorder;
static STATE: AtomicUsize = AtomicUsize::new(0);

const UNINITIALIZED: usize = 0;
const INITIALIZING: usize = 1;
const INITIALIZED: usize = 2;

static SET_RECORDER_ERROR: &str =
    "attempted to set a recorder after the metrics system was already initialized";

/// A value that records metrics behind the facade.
pub trait Recorder {
    /// Registers a counter.
    ///
    /// Callers may provide a description of the counter being registered. Whether or not a metric
    /// can be reregistered to provide a description, if one was already passed or not, as well as
    /// how descriptions are used by the underlying recorder, is an implementation detail.
    fn register_counter(&self, key: Key, description: Option<&'static str>) -> Identifier;

    /// Registers a gauge.
    ///
    /// Callers may provide a description of the counter being registered. Whether or not a metric
    /// can be reregistered to provide a description, if one was already passed or not, as well as
    /// how descriptions are used by the underlying recorder, is an implementation detail.
    fn register_gauge(&self, key: Key, description: Option<&'static str>) -> Identifier;

    /// Registers a histogram.
    ///
    /// Callers may provide a description of the counter being registered. Whether or not a metric
    /// can be reregistered to provide a description, if one was already passed or not, as well as
    /// how descriptions are used by the underlying recorder, is an implementation detail.
    fn register_histogram(&self, key: Key, description: Option<&'static str>) -> Identifier;

    /// Increments a counter.
    fn increment_counter(&self, id: Identifier, value: u64);

    /// Updates a gauge.
    fn update_gauge(&self, id: Identifier, value: f64);

    /// Records a histogram.
    ///
    /// The value can be value that implements [`IntoU64`].  By default, `metrics` provides an
    /// implementation for both `u64` itself as well as [`Duration`](std::time::Duration).
    fn record_histogram(&self, id: Identifier, value: u64);
}

struct NoopRecorder;

impl Recorder for NoopRecorder {
    fn register_counter(&self, _key: Key, _description: Option<&'static str>) -> Identifier {
        Identifier::default()
    }
    fn register_gauge(&self, _key: Key, _description: Option<&'static str>) -> Identifier {
        Identifier::default()
    }
    fn register_histogram(&self, _key: Key, _description: Option<&'static str>) -> Identifier {
        Identifier::default()
    }
    fn increment_counter(&self, _id: Identifier, _value: u64) {}
    fn update_gauge(&self, _id: Identifier, _value: f64) {}
    fn record_histogram(&self, _id: Identifier, _value: u64) {}
}

/// Sets the global recorder to a `&'static Recorder`.
///
/// This function may only be called once in the lifetime of a program.  Any metrics recorded
/// before the call to `set_recorder` occurs will be completely ignored.
///
/// This function does not typically need to be called manually.  Metrics implementations should
/// provide an initialization method that installs the recorder internally.
///
/// # Errors
///
/// An error is returned if a recorder has already been set.
#[cfg(atomic_cas)]
pub fn set_recorder(recorder: &'static dyn Recorder) -> Result<(), SetRecorderError> {
    set_recorder_inner(|| recorder)
}

/// Sets the global recorder to a `Box<Recorder>`.
///
/// This is a simple convenience wrapper over `set_recorder`, which takes a `Box<Recorder>`
/// rather than a `&'static Recorder`.  See the document for [`set_recorder`] for more
/// details.
///
/// Requires the `std` feature.
///
/// # Errors
///
/// An error is returned if a recorder has already been set.
#[cfg(all(feature = "std", atomic_cas))]
pub fn set_boxed_recorder(recorder: Box<dyn Recorder>) -> Result<(), SetRecorderError> {
    set_recorder_inner(|| unsafe { &*Box::into_raw(recorder) })
}

#[cfg(atomic_cas)]
fn set_recorder_inner<F>(make_recorder: F) -> Result<(), SetRecorderError>
where
    F: FnOnce() -> &'static dyn Recorder,
{
    unsafe {
        match STATE.compare_and_swap(UNINITIALIZED, INITIALIZING, Ordering::SeqCst) {
            UNINITIALIZED => {
                RECORDER = make_recorder();
                STATE.store(INITIALIZED, Ordering::SeqCst);
                Ok(())
            }
            INITIALIZING => {
                while STATE.load(Ordering::SeqCst) == INITIALIZING {}
                Err(SetRecorderError(()))
            }
            _ => Err(SetRecorderError(())),
        }
    }
}

/// A thread-unsafe version of [`set_recorder`].
///
/// This function is available on all platforms, even those that do not have support for atomics
/// that are needed by [`set_recorder`].
///
/// In almost all cases, [`set_recorder`] should be preferred.
///
/// # Safety
///
/// This function is only safe to call when no other metrics initialization function is called
/// while this function still executes.
///
/// This can be upheld by (for example) making sure that **there are no other threads**, and (on
/// embedded) that **interrupts are disabled**.
///
/// It is safe to use other metrics functions while this function runs (including all metrics
/// macros).
pub unsafe fn set_recorder_racy(recorder: &'static dyn Recorder) -> Result<(), SetRecorderError> {
    match STATE.load(Ordering::SeqCst) {
        UNINITIALIZED => {
            RECORDER = recorder;
            STATE.store(INITIALIZED, Ordering::SeqCst);
            Ok(())
        }
        INITIALIZING => {
            // This is just plain UB, since we were racing another initialization function
            unreachable!("set_recorder_racy must not be used with other initialization functions")
        }
        _ => Err(SetRecorderError(())),
    }
}

/// Clears the currently configured recorder.
///
/// As we give out a reference to the recorder with a static lifetime, we cannot safely reclaim
/// and drop the installed recorder when clearing.  Thus, any existing recorder will stay leaked.
///
/// This method is typically only useful for testing or benchmarking.
#[doc(hidden)]
pub fn clear_recorder() {
    STATE.store(UNINITIALIZED, Ordering::SeqCst);
}

/// The type returned by [`set_recorder`] if [`set_recorder`] has already been called.
#[derive(Debug)]
pub struct SetRecorderError(());

impl fmt::Display for SetRecorderError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.write_str(SET_RECORDER_ERROR)
    }
}

// The Error trait is not available in libcore
#[cfg(feature = "std")]
impl std::error::Error for SetRecorderError {
    fn description(&self) -> &str {
        SET_RECORDER_ERROR
    }
}

/// Returns a reference to the recorder.
///
/// If a recorder has not been set, a no-op implementation is returned.
pub fn recorder() -> &'static dyn Recorder {
    static NOOP: NoopRecorder = NoopRecorder;
    try_recorder().unwrap_or(&NOOP)
}

/// Returns a reference to the recorder.
///
/// If a recorder has not been set, returns `None`.
pub fn try_recorder() -> Option<&'static dyn Recorder> {
    unsafe {
        if STATE.load(Ordering::SeqCst) != INITIALIZED {
            None
        } else {
            Some(RECORDER)
        }
    }
}
