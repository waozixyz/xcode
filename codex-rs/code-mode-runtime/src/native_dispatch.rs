use codex_code_mode_protocol::ToolDefinition;

#[repr(C)]
struct Tool<'a> {
    name: *const u8,
    length: i64,
    kind: i32,
    lifetime: std::marker::PhantomData<&'a str>,
}

unsafe extern "C" {
    fn xcoder_resolve_tool(name: *const u8, length: i64, tools: *const Tool<'_>, count: i64)
    -> i64;
    fn xcoder_executor_init(state: *mut Executor, count: i64, lanes: i64) -> i32;
    fn xcoder_executor_next(state: *mut Executor) -> i64;
    fn xcoder_executor_complete(state: *mut Executor) -> i32;
}

#[repr(C)]
#[derive(Default)]
pub(crate) struct Executor {
    cursor: i64,
    active: i64,
    completed: i64,
    count: i64,
    lanes: i64,
}

impl Executor {
    pub(crate) fn new(count: usize, parallel: bool) -> Result<Self, String> {
        let mut state = Self::default();
        let count = i64::try_from(count).map_err(|_| "too many plan calls")?;
        // SAFETY: Ziran mutates only this valid, exclusively borrowed C struct.
        if unsafe { xcoder_executor_init(&mut state, count, if parallel { 4 } else { 1 }) } == 0 {
            return Err("a tool plan supports at most 128 calls".into());
        }
        Ok(state)
    }

    pub(crate) fn next(&mut self) -> Option<usize> {
        // SAFETY: exclusively borrowed state initialized by the native executor.
        usize::try_from(unsafe { xcoder_executor_next(self) }).ok()
    }

    pub(crate) fn complete(&mut self) {
        // SAFETY: exclusively borrowed initialized state; one call per reply.
        assert_eq!(unsafe { xcoder_executor_complete(self) }, 1);
    }

    pub(crate) fn done(&self) -> bool {
        self.completed == self.count
    }
}

pub(crate) struct Catalog<'a> {
    definitions: &'a [ToolDefinition],
    entries: Vec<Tool<'a>>,
}

// SAFETY: entries point only into immutable borrowed strings. The C ABI retains
// nothing and performs no mutation. The borrow outlives every native lookup.
unsafe impl Send for Catalog<'_> {}
unsafe impl Sync for Catalog<'_> {}

impl<'a> Catalog<'a> {
    pub(crate) fn new(definitions: &'a [ToolDefinition]) -> Self {
        let entries = definitions
            .iter()
            .map(|tool| Tool {
                name: tool.name.as_ptr(),
                length: tool.name.len() as i64,
                kind: 0,
                lifetime: std::marker::PhantomData,
            })
            .collect();
        Self {
            definitions,
            entries,
        }
    }

    pub(crate) fn resolve(&self, name: &str) -> Option<&'a ToolDefinition> {
        // SAFETY: all pointers borrow immutable slices held for this call. Ziran
        // checks lengths and returns an index; it retains no pointers or ownership.
        let index = unsafe {
            xcoder_resolve_tool(
                name.as_ptr(),
                name.len() as i64,
                self.entries.as_ptr(),
                self.entries.len() as i64,
            )
        };
        usize::try_from(index)
            .ok()
            .and_then(|index| self.definitions.get(index))
    }
}
