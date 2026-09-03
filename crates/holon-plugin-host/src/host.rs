//! Generic wasmi host for the five-function plugin ABI.
//!
//! The guest is a pure function: bytes plus a JSON context in, JSON Lines out.
//! No imports are provided at all, so a guest that tries to reach WASI, the
//! clock or the network fails to INSTANTIATE rather than silently degrading.
//!
//! One [`PluginHost`] holds one instantiated module and is reused across
//! files: instantiation costs milliseconds and buys nothing per call.

use wasmi::Config;
use wasmi::Engine;
use wasmi::Instance;
use wasmi::Linker;
use wasmi::Memory;
use wasmi::Module;
use wasmi::Store;
use wasmi::StoreLimits;
use wasmi::StoreLimitsBuilder;
use wasmi::TrapCode;
use wasmi::TypedFunc;

use crate::abi;

/// What a guest is allowed to spend on one call.
///
/// Both are hard stops rather than budgets: a plugin the user dropped into
/// their config directory is untrusted code, and an unbounded loop or an
/// unbounded allocation in one must end the CALL, not the application.
#[derive(Debug, Clone, Copy)]
pub struct PluginLimits {
    /// Fuel granted per `parse` call. Fuel is refilled before each call, so
    /// this bounds one parse rather than the host's lifetime.
    pub fuel_per_call: u64,
    /// Ceiling on the guest's linear memory, in bytes.
    pub memory_bytes: usize,
}

impl Default for PluginLimits {
    /// Measured, not guessed: the cooklang guest spends 150.7 M fuel and
    /// 2.2 MiB on a 200-step recipe — far larger than any real one — so these
    /// leave a 6.6x and a 28x margin, which `fuel_and_memory_headroom` pins.
    /// At the measured ~1.1 G fuel/s a runaway guest is stopped inside a
    /// second.
    fn default() -> Self {
        Self {
            fuel_per_call: 1_000_000_000,
            memory_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub enum PluginError {
    Wasm(String),
    MissingExport(&'static str),
    /// The guest refused the input and named why. DATA about the file, not a
    /// defect in the plugin.
    GuestError(String),
    OutOfFuel {
        fuel_per_call: u64,
    },
    OutOfMemory {
        memory_bytes: usize,
    },
    OutOfBounds {
        ptr: u32,
        len: u32,
        mem: usize,
    },
    Utf8(String),
}

impl core::fmt::Display for PluginError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PluginError::Wasm(m) => write!(f, "wasm error: {m}"),
            PluginError::MissingExport(e) => write!(f, "guest is missing required export `{e}`"),
            PluginError::GuestError(m) => write!(f, "guest reported: {m}"),
            PluginError::OutOfFuel { fuel_per_call } => write!(
                f,
                "guest exhausted its {fuel_per_call} fuel for this call without returning; it is \
                 looping or the input is far larger than this plugin was budgeted for"
            ),
            PluginError::OutOfMemory { memory_bytes } => write!(
                f,
                "guest tried to grow past its {memory_bytes}-byte memory limit"
            ),
            PluginError::OutOfBounds { ptr, len, mem } => write!(
                f,
                "guest returned a slice ({ptr}..{}) outside its {mem}-byte memory",
                *ptr as u64 + *len as u64
            ),
            PluginError::Utf8(m) => write!(f, "guest output is not UTF-8: {m}"),
        }
    }
}

impl std::error::Error for PluginError {}

pub struct PluginHost {
    store: Store<StoreLimits>,
    limits: PluginLimits,
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    dealloc: TypedFunc<(u32, u32), ()>,
    parse: TypedFunc<(u32, u32, u32, u32), u64>,
    last_error: TypedFunc<(), u64>,
    live_bytes: TypedFunc<(), u64>,
}

impl PluginHost {
    pub fn from_bytes(wasm: &[u8], limits: PluginLimits) -> Result<Self, PluginError> {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, wasm).map_err(|e| PluginError::Wasm(e.to_string()))?;

        let store_limits = StoreLimitsBuilder::new()
            .memory_size(limits.memory_bytes)
            // Otherwise `memory.grow` returns -1 and the guest's allocator
            // decides what to do about it — a refusal we would never see.
            .trap_on_grow_failure(true)
            .build();
        let mut store = Store::new(&engine, store_limits);
        store.limiter(|limits| limits);
        set_fuel(&mut store, limits.fuel_per_call)?;

        let linker = <Linker<StoreLimits>>::new(&engine);
        let instance: Instance = linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|e| PluginError::Wasm(e.to_string()))?;

        let memory = instance
            .get_memory(&store, abi::EXPORT_MEMORY)
            .ok_or(PluginError::MissingExport(abi::EXPORT_MEMORY))?;
        let alloc = typed(&instance, &store, abi::EXPORT_ALLOC)?;
        let dealloc = typed(&instance, &store, abi::EXPORT_DEALLOC)?;
        let parse = typed(&instance, &store, abi::EXPORT_PARSE)?;
        let last_error = typed(&instance, &store, abi::EXPORT_LAST_ERROR)?;
        let live_bytes = typed(&instance, &store, abi::EXPORT_LIVE_BYTES)?;

        Ok(Self {
            store,
            limits,
            memory,
            alloc,
            dealloc,
            parse,
            last_error,
            live_bytes,
        })
    }

    /// Run the guest's `holon_parse` over `input` with `ctx` (JSON) and return
    /// the JSON Lines it emitted.
    pub fn parse(&mut self, input: &[u8], ctx: &[u8]) -> Result<String, PluginError> {
        set_fuel(&mut self.store, self.limits.fuel_per_call)?;

        let mut call = CallScope::new(self);
        let (in_ptr, in_len) = call.lend(input)?;
        let (ctx_ptr, ctx_len) = call.lend(ctx)?;
        let packed = call.run(in_ptr, in_len, ctx_ptr, ctx_len)?;
        if packed == 0 {
            return Err(call.host.guest_error());
        }
        let (out_ptr, out_len) = abi::unpack(packed);
        let bytes = call.host.read_out(out_ptr, out_len)?;
        // Only once the span read back: returning one the guest never handed
        // out would corrupt its allocator rather than free anything.
        call.owns(out_ptr, out_len);
        call.release()?;

        String::from_utf8(bytes).map_err(|e| PluginError::Utf8(e.to_string()))
    }

    /// Bytes the guest lent this host and has not got back. Zero between
    /// calls; anything else is a buffer this host forgot to release.
    pub fn guest_live_bytes(&mut self) -> Result<u64, PluginError> {
        self.live_bytes
            .call(&mut self.store, ())
            .map_err(|e| self.classify(e))
    }

    /// Fuel the last call consumed. The margin between this and
    /// [`PluginLimits::fuel_per_call`] is what a test can assert on.
    pub fn fuel_remaining(&self) -> u64 {
        self.store.get_fuel().unwrap_or(0)
    }

    /// The guest's current linear-memory size in bytes.
    pub fn memory_bytes(&self) -> usize {
        self.memory.data(&self.store).len()
    }

    /// Copy `bytes` into a freshly allocated guest buffer and return its span.
    fn write_in(&mut self, bytes: &[u8]) -> Result<(u32, u32), PluginError> {
        let len = bytes.len() as u32;
        let ptr = self
            .alloc
            .call(&mut self.store, len)
            .map_err(|e| self.classify(e))?;
        self.memory
            .write(&mut self.store, ptr as usize, bytes)
            .map_err(|e| PluginError::Wasm(e.to_string()))?;
        Ok((ptr, len))
    }

    fn free(&mut self, ptr: u32, len: u32) -> Result<(), PluginError> {
        self.dealloc
            .call(&mut self.store, (ptr, len))
            .map_err(|e| self.classify(e))
    }

    fn read_out(&self, ptr: u32, len: u32) -> Result<Vec<u8>, PluginError> {
        let data = self.memory.data(&self.store);
        let end = ptr as usize + len as usize;
        if end > data.len() {
            return Err(PluginError::OutOfBounds {
                ptr,
                len,
                mem: data.len(),
            });
        }
        Ok(data[ptr as usize..end].to_vec())
    }

    /// A trap that a limit caused is reported as that limit, so a looping
    /// guest never reads as "wasm error" beside a genuine one.
    fn classify(&self, error: wasmi::Error) -> PluginError {
        match error.as_trap_code() {
            Some(TrapCode::OutOfFuel) => PluginError::OutOfFuel {
                fuel_per_call: self.limits.fuel_per_call,
            },
            Some(TrapCode::GrowthOperationLimited) => PluginError::OutOfMemory {
                memory_bytes: self.limits.memory_bytes,
            },
            _ => PluginError::Wasm(error.to_string()),
        }
    }

    fn guest_error(&mut self) -> PluginError {
        let packed = match self.last_error.call(&mut self.store, ()) {
            Ok(packed) => packed,
            Err(e) => return self.classify(e),
        };
        let (ptr, len) = abi::unpack(packed);
        match self.read_out(ptr, len) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(message) => PluginError::GuestError(message),
                Err(e) => PluginError::Utf8(e.to_string()),
            },
            Err(e) => e,
        }
    }
}

/// The guest buffers of ONE `parse`, released down every path out of it.
///
/// A trap is such a path: without this, each trapped call left its input in a
/// guest that outlives the call, so a vault full of files one plugin refuses
/// by trapping would walk the host into its own memory ceiling.
struct CallScope<'h> {
    host: &'h mut PluginHost,
    lent: Vec<(u32, u32)>,
}

impl<'h> CallScope<'h> {
    fn new(host: &'h mut PluginHost) -> Self {
        Self {
            host,
            lent: Vec::new(),
        }
    }

    fn lend(&mut self, bytes: &[u8]) -> Result<(u32, u32), PluginError> {
        let span = self.host.write_in(bytes)?;
        self.owns(span.0, span.1);
        Ok(span)
    }

    fn owns(&mut self, ptr: u32, len: u32) {
        self.lent.push((ptr, len));
    }

    fn run(
        &mut self,
        in_ptr: u32,
        in_len: u32,
        ctx_ptr: u32,
        ctx_len: u32,
    ) -> Result<u64, PluginError> {
        let host = &mut *self.host;
        host.parse
            .call(&mut host.store, (in_ptr, in_len, ctx_ptr, ctx_len))
            .map_err(|e| host.classify(e))
    }

    /// Release on the ordinary path, where a failing `holon_dealloc` is the
    /// caller's to see rather than the drop glue's to log.
    fn release(&mut self) -> Result<(), PluginError> {
        for (ptr, len) in std::mem::take(&mut self.lent) {
            self.host.free(ptr, len)?;
        }
        Ok(())
    }
}

impl Drop for CallScope<'_> {
    fn drop(&mut self) {
        if self.lent.is_empty() {
            return;
        }
        // Whatever ended the call may have been fuel exhaustion, and the
        // release is guest code that needs a grant of its own.
        let refuelled = set_fuel(&mut self.host.store, self.host.limits.fuel_per_call);
        if let Err(e) = refuelled.and_then(|()| self.release()) {
            tracing::error!(
                error = %e,
                "guest kept its buffers: the release after a failed parse failed too"
            );
        }
    }
}

fn set_fuel(store: &mut Store<StoreLimits>, fuel: u64) -> Result<(), PluginError> {
    store
        .set_fuel(fuel)
        .map_err(|e| PluginError::Wasm(format!("cannot set fuel: {e}")))
}

fn typed<P, R>(
    instance: &Instance,
    store: &Store<StoreLimits>,
    name: &'static str,
) -> Result<TypedFunc<P, R>, PluginError>
where
    P: wasmi::WasmParams,
    R: wasmi::WasmResults,
{
    instance
        .get_func(store, name)
        .ok_or(PluginError::MissingExport(name))?
        .typed(store)
        .map_err(|e| PluginError::Wasm(format!("{name}: {e}")))
}

impl core::fmt::Debug for PluginHost {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PluginHost")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}
