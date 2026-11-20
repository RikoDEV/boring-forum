//! Running a hook in one plugin: the module call itself and the fault accounting around it.

use crate::state::AppState;
use bf_db::platform::plugins;
use serde_json::{Value, json};
use std::sync::Arc;
use wasmtime::{Caller, Store, StoreLimitsBuilder};

use super::{
    ACTIVE, CALL_DEADLINE, Code, ENGINE, FUEL_PER_CALL, HTTP_PREFIX, HostState, Loaded, MAX_FAILURES, MEMORY_LIMIT,
    TABLE_ELEMENTS, linker, reload, stats,
};

/// Hooks that a visitor's request drives, with input of their choosing. A failure there says
/// nothing about the plugin, so it must not count towards switching it off for everyone.
fn visitor_driven(hook: &str) -> bool {
    matches!(
        hook,
        "api.request"
            | "panel.render"
            | "login.begin"
            | "login.callback"
            | "slot.render"
            | "onebox.resolve"
            | "action.run"
            | "page.request"
    )
}

/// Copies bytes into the module through its own `alloc`.
pub(super) fn write_out(c: &mut Caller<'_, HostState>, bytes: &[u8]) -> Result<i64, wasmtime::Error> {
    let alloc =
        c.get_export("alloc").and_then(wasmtime::Extern::into_func).ok_or_else(|| wasmtime::Error::msg("no alloc"))?;
    let alloc = alloc.typed::<i32, i32>(&*c)?;
    let ptr = alloc.call(&mut *c, bytes.len() as i32)?;
    let memory = c
        .get_export("memory")
        .and_then(wasmtime::Extern::into_memory)
        .ok_or_else(|| wasmtime::Error::msg("no memory"))?;
    memory.write(&mut *c, ptr as usize, bytes)?;
    Ok((i64::from(ptr) << 32) | bytes.len() as i64)
}

pub(super) fn run_module(
    p: &Loaded,
    state: Option<AppState>,
    pool: sqlx::PgPool,
    handle: tokio::runtime::Handle,
    input: &[u8],
) -> Result<Value, wasmtime::Error> {
    let Code::Wasm(module) = &p.code else { return Err(wasmtime::Error::msg("not a module")) };
    let host = HostState {
        limits: StoreLimitsBuilder::new()
            .memory_size(MEMORY_LIMIT)
            .table_elements(TABLE_ELEMENTS)
            .instances(1)
            .tables(4)
            .memories(1)
            .build(),
        plugin: p.id.clone(),
        state,
        capabilities: p.capabilities.clone(),
        tables: p.tables.clone(),
        catalogs: p.catalogs.clone(),
        writes: 0,
        can_kv: p.capabilities.iter().any(|c| c == "kv"),
        http_hosts: p.capabilities.iter().filter_map(|c| c.strip_prefix(HTTP_PREFIX)).map(str::to_owned).collect(),
        http_calls: 0,
        pool,
        handle,
    };
    let mut store = Store::new(&ENGINE, host);
    store.limiter(|h| &mut h.limits);
    store.set_fuel(FUEL_PER_CALL)?;
    store.set_epoch_deadline(CALL_DEADLINE);
    let result = invoke(&mut store, module, input);
    // Fuel is spent however the call ends, so a plugin that traps is still measured.
    stats::record_fuel(&p.id, FUEL_PER_CALL.saturating_sub(store.get_fuel().unwrap_or(0)));
    result
}

fn invoke(store: &mut Store<HostState>, module: &wasmtime::Module, input: &[u8]) -> Result<Value, wasmtime::Error> {
    let instance = linker()?.instantiate(&mut *store, module)?;
    let memory = instance.get_memory(&mut *store, "memory").ok_or_else(|| wasmtime::Error::msg("no memory export"))?;
    let alloc = instance.get_typed_func::<i32, i32>(&mut *store, "alloc")?;
    let handle_fn = instance.get_typed_func::<(i32, i32), i64>(&mut *store, "handle")?;
    let ptr = alloc.call(&mut *store, input.len() as i32)?;
    memory.write(&mut *store, ptr as usize, input)?;
    let out = handle_fn.call(&mut *store, (ptr, input.len() as i32))?;
    let (out_ptr, out_len) = (((out >> 32) & 0xffff_ffff) as usize, (out & 0xffff_ffff) as usize);
    if out_len > 1024 * 1024 {
        return Err(wasmtime::Error::msg("output too large"));
    }
    let bytes = memory
        .data(&*store)
        .get(out_ptr..out_ptr + out_len)
        .ok_or_else(|| wasmtime::Error::msg("output out of bounds"))?;
    Ok(serde_json::from_slice(bytes)?)
}

/// Runs a hook in one plugin. Faults are counted; too many disable the plugin and tell admins.
pub(super) async fn call(state: &AppState, p: Arc<Loaded>, hook: &str, data: &Value) -> Option<Value> {
    // Plugin code runs on blocking threads and holds a database connection while it works; hooks
    // fired by anonymous requests must not be able to pile up without end.
    static RUNNING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(32);
    let Ok(_slot) = tokio::time::timeout(std::time::Duration::from_secs(2), RUNNING.acquire()).await else {
        tracing::warn!(plugin = %p.id, hook, "plugin hook skipped: too many running");
        return None;
    };
    let _slot = _slot.ok()?;
    let pool = state.pool.clone()?;
    let input = serde_json::to_vec(&json!({ "hook": hook, "data": data, "settings": p.settings })).ok()?;
    let (plugin, worker_pool, handle) = (p.clone(), pool.clone(), tokio::runtime::Handle::current());
    let (worker_state, hook_name, native_data) = (state.clone(), hook.to_owned(), data.clone());
    let result = tokio::task::spawn_blocking(move || match &plugin.code {
        Code::Wasm(_) => run_module(&plugin, Some(worker_state), worker_pool, handle, &input),
        Code::Native(n) => n.handle(&hook_name, &native_data).map_err(wasmtime::Error::msg),
    })
    .await;
    match result {
        Ok(Ok(v)) => {
            stats::record_call(&p.id, None);
            Some(v)
        }
        Ok(Err(e)) => {
            tracing::warn!(plugin = %p.id, hook, error = %e, "plugin call failed");
            stats::record_call(&p.id, Some(&format!("{hook}: {e}")));
            if visitor_driven(hook) {
                return None;
            }
            if plugins::record_failure(&pool, &p.id).await.is_ok_and(|n| n >= MAX_FAILURES) {
                let _ = plugins::disable(&pool, &p.id).await;
                let text = format!("Plugin {} kept failing and was disabled", p.id);
                let _ = bf_db::admin::notices::add_notice(&pool, "plugin", &text).await;
                let _ = reload(state).await;
            }
            None
        }
        Err(e) => {
            tracing::error!(plugin = %p.id, error = %e, "plugin task failed");
            None
        }
    }
}

pub(super) fn listening(hook: &str) -> Vec<Arc<Loaded>> {
    let active = ACTIVE.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    active.iter().filter(|p| p.hooks.iter().any(|h| h == hook)).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::plugins::{NATIVE, Plugin, register_native};
    use wasmtime::{Linker, Module};

    fn loaded(wat: &str) -> Loaded {
        Loaded {
            id: "t".into(),
            code: Code::Wasm(Module::new(&ENGINE, wat).unwrap()),
            capabilities: vec![],
            hooks: vec![],
            settings: Value::Null,
            tables: Arc::default(),
            panels: vec![],
            routes: vec![],
            types: vec![],
            catalogs: Arc::default(),
        }
    }

    const BASE: &str =
        "(memory (export \"memory\") 1) (func (export \"alloc\") (param i32) (result i32) (i32.const 1024))";

    fn run(p: &Loaded) -> Result<Value, wasmtime::Error> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let handle = rt.handle().clone();
        let pool = {
            let _guard = rt.enter();
            sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://x@127.0.0.1:1/x").unwrap()
        };
        run_module(p, None, pool, handle, b"{}")
    }

    #[test]
    fn endless_loops_run_out_of_fuel() {
        let p = loaded(&format!(
            "(module {BASE} (func (export \"handle\") (param i32 i32) (result i64) (loop $l (br $l)) (i64.const 0)))"
        ));
        assert!(run(&p).is_err());
    }

    #[test]
    fn tables_are_capped() {
        // Growing a table by more than the limit is refused (-1), which this module turns into a trap
        // if it ever succeeds.
        let p = loaded(&format!(
            "(module (table 1 funcref) {BASE} (func (export \"handle\") (param i32 i32) (result i64) (if (i32.ne (table.grow (ref.null func) (i32.const 1000000)) (i32.const -1)) (then unreachable)) (i64.const 0)))"
        ));
        // The grow is refused, then the module returns no JSON: an error either way, but not the trap.
        let err = run(&p).unwrap_err().to_string();
        assert!(!err.contains("unreachable"), "{err}");
    }

    #[test]
    fn only_visitor_driven_hooks_skip_the_failure_count() {
        assert!(visitor_driven("api.request") && visitor_driven("panel.render"));
        assert!(!visitor_driven("post.created") && !visitor_driven("job.run"));
    }

    #[test]
    fn traps_are_errors() {
        let p = loaded(&format!("(module {BASE} (func (export \"handle\") (param i32 i32) (result i64) unreachable))"));
        assert!(run(&p).is_err());
    }

    #[test]
    fn memory_is_capped() {
        let p = loaded(&format!(
            "(module {BASE} (func (export \"handle\") (param i32 i32) (result i64) (drop (memory.grow (i32.const 60000))) (i64.const 0)))"
        ));
        // Growing is refused, so the module sees failure rather than getting 4 GB.
        assert!(run(&p).is_err());
    }

    #[test]
    fn host_requests_need_their_capability() {
        let mut p = loaded(&format!(
            "(module (import \"bf\" \"request\" (func $r (param i32 i32) (result i64))) {BASE} (data (i32.const 0) \"{{\\\"op\\\":\\\"notify\\\"}}\") (func (export \"handle\") (param i32 i32) (result i64) (if (i64.eq (call $r (i32.const 0) (i32.const 15)) (i64.const -1)) (then unreachable)) (i64.const 0)))"
        ));
        assert!(run(&p).is_err(), "no capability: the host answers -1");
        p.capabilities = vec!["kv".into()];
        assert!(run(&p).is_err(), "another capability does not help");
    }

    struct Echo;
    impl Plugin for Echo {
        fn id(&self) -> &str {
            "echo"
        }
        fn hooks(&self) -> Vec<String> {
            vec!["post.created".into()]
        }
        fn handle(&self, hook: &str, data: &Value) -> Result<Value, String> {
            Ok(json!({ "hook": hook, "data": data }))
        }
    }

    #[test]
    fn native_plugins_register_and_are_not_run_as_modules() {
        register_native(Arc::new(Echo));
        let registered = NATIVE.read().unwrap().iter().find(|n| n.id() == "echo").cloned().unwrap();
        assert_eq!(registered.hooks(), ["post.created"]);
        assert_eq!(registered.handle("post.created", &json!(1)).unwrap()["data"], 1);
        let mut p = loaded("(module)");
        p.code = Code::Native(registered);
        assert!(run(&p).is_err());
    }

    #[test]
    fn modules_have_no_ambient_imports() {
        // Anything outside the bf functions, such as WASI, cannot even be linked.
        let m = Module::new(
            &ENGINE,
            "(module (import \"wasi_snapshot_preview1\" \"fd_write\" (func (param i32 i32 i32 i32) (result i32))))",
        )
        .unwrap();
        let mut store = Store::new(&ENGINE, ());
        assert!(Linker::<()>::new(&ENGINE).instantiate(&mut store, &m).is_err());
    }
}
