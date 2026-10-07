//! qoder WASM 加密桥（feature = "wasm"，wasmtime 接线）。
//!
//! 复刻参考实现 deepseek-harness-codearts/src/qoder-wasm.ts（706 行）的
//! wasm-bindgen ABI：
//! - **对象堆**：索引 0..1023 Undefined + 哨兵（1024 Undefined / 1025 Null /
//!   1026 True / 1027 False）；≥1028 走 free-list 回收。
//! - **callString 布局**（栈帧 16 字节小端 i32×4）：`[ptr, len, errObj, isError]`；
//!   callPointer 用 `[ptr, errObj, isError]`。
//! - **`requestresult_url(栈指针, ptr)` 参数顺序与直觉相反**（栈指针在前）。
//! - **headers 必须原样透传**：`Authorization: Bearer COSY.<载荷>.<签名>`
//!   由 WASM 生成，覆盖会 `403 Signature invalid`。
//! - 宿主环境探测走 **crypto.getRandomValues** 路径（process/require/
//!   WINDOW/SELF/GLOBAL 以 Undefined 断链）。
//! - import 签名（31 个）全部由 wasm 二进制 import 段解析核对。
//!
//! 本类型**非线程安全**（与参考一致），一个账号一个实例。

#![cfg(feature = "wasm")]

use wasmtime::{Caller, Engine, FuncType, Linker, Memory, Module, Store, TypedFunc, Val, ValType};

const WASM_BYTES: &[u8] = include_bytes!("../../wasm/qoder-auth-wasm.wasm");
/// import 模块名必须逐字一致（qoder-wasm.ts:72）。
const IMPORT_MODULE: &str = "./qoder_auth_wasm_bg.js";
/// 动态对象堆起始索引（0..1027 为哨兵区）。
const FIRST_FREE: u32 = 1028;

#[derive(Debug, thiserror::Error)]
pub enum WasmError {
    #[error("wasm trap: {0}")]
    Trap(String),
    #[error("wasm 侧错误: {0}")]
    Wasm(String),
    #[error("abi 违约: {0}")]
    Abi(String),
}

/// JS 对象堆值的 Rust 侧形态。
#[derive(Debug, Clone)]
#[allow(dead_code)] // Bool 哨兵仅在类型判定时语义存在
enum HostVal {
    Undefined,
    Null,
    Bool(bool),
    Bytes(Vec<u8>),
    Str(String),
    /// globalThis 占位（带 crypto 成员）
    GlobalThis,
    /// crypto 占位（带 getRandomValues）
    Crypto,
    /// JS Map（requestresult_headers 返回形态）
    Map(Vec<(String, String)>),
}

impl HostVal {
    fn is_undefined(&self) -> bool {
        matches!(self, HostVal::Undefined)
    }
    fn is_string(&self) -> bool {
        matches!(self, HostVal::Str(_))
    }
    fn is_object(&self) -> bool {
        matches!(
            self,
            HostVal::Bytes(_) | HostVal::Str(_) | HostVal::Map(_) | HostVal::GlobalThis | HostVal::Crypto
        )
    }
}

/// ≥1028 的动态对象堆（free-list 回收）。
struct ObjHeap {
    vals: Vec<HostVal>,
    free: std::collections::VecDeque<u32>,
}

impl ObjHeap {
    /// 动态堆索引从 FIRST_FREE(1028) 起——0..1027 是哨兵区，二者不可重叠。
    fn push(&mut self, v: HostVal) -> u32 {
        if let Some(i) = self.free.pop_front() {
            self.vals[i as usize] = v;
            FIRST_FREE + i
        } else {
            self.vals.push(v);
            FIRST_FREE + (self.vals.len() - 1) as u32
        }
    }
    fn slot(&self, idx: u32) -> usize {
        (idx - FIRST_FREE) as usize
    }
    /// 原位替换（JS：heapObject(i).mutate() 后索引不变，wasm 继续持有）
    fn replace(&mut self, i: u32, v: HostVal) {
        let s = self.slot(i);
        if let Some(slot) = self.vals.get_mut(s) {
            *slot = v;
        }
    }
    fn take(&mut self, i: u32) -> HostVal {
        let s = self.slot(i);
        let v = self.vals.get(s).cloned().unwrap_or(HostVal::Undefined);
        if s < self.vals.len() {
            self.vals[s] = HostVal::Undefined;
            self.free.push_back(s as u32);
        }
        v
    }
}

struct Ctx {
    /// 哨兵区 0..1028 固定（1024 Undefined/1025 Null/1026 True/1027 False）。
    sentinels: Vec<HostVal>,
    heap: ObjHeap,
}

impl Ctx {
    fn new() -> Self {
        let mut sentinels = vec![HostVal::Undefined; 1024];
        sentinels.push(HostVal::Undefined);
        sentinels.push(HostVal::Null);
        sentinels.push(HostVal::Bool(true));
        sentinels.push(HostVal::Bool(false));
        Self { sentinels, heap: ObjHeap { vals: Vec::new(), free: std::collections::VecDeque::new() } }
    }
    fn val(&self, i: u32) -> HostVal {
        if (i as usize) < self.sentinels.len() {
            self.sentinels[i as usize].clone()
        } else {
            self.heap.vals.get(self.heap.slot(i)).cloned().unwrap_or(HostVal::Undefined)
        }
    }
    fn heap_replace(&mut self, idx: u32, v: HostVal) {
        if (idx as usize) >= self.sentinels.len() {
            self.heap.replace(idx, v);
        }
    }
    fn take(&mut self, i: u32) -> HostVal {
        if (i as usize) < self.sentinels.len() {
            self.sentinels[i as usize].clone()
        } else {
            self.heap.take(i)
        }
    }
    fn push(&mut self, v: HostVal) -> u32 {
        self.heap.push(v)
    }
}

/// 加密推理请求（headers 必须原样透传——Authorization 由 WASM 签名生成）。
#[derive(Debug, Clone)]
pub struct InferRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// 运行时鉴权字段。
#[derive(Debug, Clone)]
pub struct RuntimeAuthFields {
    pub encrypt_user_info: String,
    pub key: String,
}

struct Frame {
    ptr: i32,
    len: i32,
    err_obj: u32,
    is_err: i32,
}

/// wasm-bindgen 长参数签名（ABI 布局固定，无法拆分）。
type Ctx9 = TypedFunc<(i32, i32, i32, i32, i32, i32, i32, i32, i32), ()>;
type Ctx10 = TypedFunc<(i32, i32, i32, i32, i32, i32, i32, i32, i32, i32), ()>;

struct Funcs {
    add_to_stack: TypedFunc<(i32,), i32>,
    realloc_str: TypedFunc<(i32, i32), i32>,
    dealloc: TypedFunc<(i32, i32, i32), ()>,
    generate_auth_fields: TypedFunc<(i32, i32, i32), ()>,
    context_new: Ctx9,
    prepare_infer: Ctx10,
    result_headers: TypedFunc<i32, i32>,
    result_url: TypedFunc<(i32, i32), ()>,
    result_body: TypedFunc<(i32, i32), ()>,
}

pub struct QoderWasm {
    store: Store<Ctx>,
    funcs: Funcs,
    memory: Memory,
}

impl QoderWasm {
    pub fn new() -> Result<Self, WasmError> {
        let engine = Engine::default();
        let module = Module::new(&engine, WASM_BYTES).map_err(|e| WasmError::Abi(e.to_string()))?;
        let mut linker: Linker<Ctx> = Linker::new(&engine);
        link_all(&mut linker)?;
        let mut store = Store::new(&engine, Ctx::new());
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| WasmError::Abi(e.to_string()))?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| WasmError::Abi("缺 memory 导出".into()))?;
        macro_rules! g {
            ($name:literal) => {
                instance
                    .get_typed_func(&mut store, $name)
                    .map_err(|e| WasmError::Abi(format!("{}: {e}", $name)))?
            };
        }
        let funcs = Funcs {
            add_to_stack: g!("__wbindgen_add_to_stack_pointer"),
            realloc_str: g!("__wbindgen_export2"),
            dealloc: g!("__wbindgen_export4"),
            generate_auth_fields: g!("generate_runtime_auth_fields"),
            context_new: g!("qodercontext_new"),
            prepare_infer: g!("qodercontext_prepareInferRequest"),
            result_headers: g!("requestresult_headers"),
            result_url: g!("requestresult_url"),
            result_body: g!("requestresult_body"),
        };
        Ok(Self { store, funcs, memory })
    }

    fn write_string(&mut self, text: &str) -> Result<(i32, i32), WasmError> {
        let encoded = text.as_bytes();
        let ptr = self
            .funcs
            .realloc_str
            .call(&mut self.store, (encoded.len() as i32, 1))
            .map_err(|e| WasmError::Trap(e.to_string()))?;
        if ptr < 0 {
            return Err(WasmError::Abi("realloc 返回负指针".into()));
        }
        self.memory
            .write(&mut self.store, ptr as usize, encoded)
            .map_err(|e| WasmError::Abi(e.to_string()))?;
        Ok((ptr, encoded.len() as i32))
    }

    fn read_string(&mut self, ptr: i32, len: i32) -> String {
        if ptr <= 0 || len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u8; len as usize];
        let _ = self.memory.read(&mut self.store, ptr as usize, &mut buf);
        String::from_utf8_lossy(&buf).to_string()
    }

    /// callString：栈帧 [ptr, len, errObj, isError]；成功后 dealloc 字符串。
    fn call_string(
        &mut self,
        invoke: impl FnOnce(&mut Self, i32) -> Result<(), WasmError>,
    ) -> Result<String, WasmError> {
        let stack = self.stack_alloc(16)?;
        let outcome = (|| {
            invoke(self, stack)?;
            let f = self.read_frame(stack);
            if f.is_err != 0 {
                let msg = match self.store.data_mut().take(f.err_obj) {
                    HostVal::Str(s) => s,
                    other => format!("{other:?}"),
                };
                return Err(WasmError::Wasm(msg));
            }
            let s = self.read_string(f.ptr, f.len);
            if f.ptr != 0 {
                let _ = self.funcs.dealloc.call(&mut self.store, (f.ptr, f.len, 1));
            }
            Ok(s)
        })();
        self.stack_free(16);
        outcome
    }

    /// callPointer：栈帧 [ptr, errObj, isError]（ptr 是对象/上下文索引）。
    fn call_pointer(
        &mut self,
        invoke: impl FnOnce(&mut Self, i32) -> Result<(), WasmError>,
    ) -> Result<i32, WasmError> {
        let stack = self.stack_alloc(16)?;
        let outcome = (|| {
            invoke(self, stack)?;
            let f = self.read_frame(stack);
            if f.is_err != 0 {
                let msg = match self.store.data_mut().take(f.err_obj) {
                    HostVal::Str(s) => s,
                    other => format!("{other:?}"),
                };
                return Err(WasmError::Wasm(msg));
            }
            Ok(f.ptr)
        })();
        self.stack_free(16);
        outcome
    }

    fn stack_alloc(&mut self, delta: i32) -> Result<i32, WasmError> {
        self.funcs
            .add_to_stack
            .call(&mut self.store, (-delta,))
            .map_err(|e| WasmError::Trap(e.to_string()))
    }

    fn stack_free(&mut self, delta: i32) {
        let _ = self.funcs.add_to_stack.call(&mut self.store, (delta,));
    }

    fn read_frame(&mut self, stack: i32) -> Frame {
        let mut buf = [0u8; 16];
        let _ = self.memory.read(&mut self.store, stack as usize, &mut buf);
        let le = |o: usize| i32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
        Frame { ptr: le(0), len: le(4), err_obj: le(8) as u32, is_err: le(12) }
    }

    // ── 对外 API（对齐 qoder-wasm.ts 调用链） ──

    /// 生成运行时鉴权字段（qoder-wasm.ts:550-564）。
    pub fn generate_runtime_auth_fields(
        &mut self,
        uid: &str,
        security_oauth_token: &str,
    ) -> Result<RuntimeAuthFields, WasmError> {
        let payload = serde_json::json!({
            "uid": uid,
            "security_oauth_token": security_oauth_token,
            "organization_id": "",
            "organization_tags": [],
            "data_policy_agreed": false,
        })
        .to_string();
        let raw = self.call_string(|me, stack| {
            let (ptr, len) = me.write_string(&payload)?;
            me.funcs
                .generate_auth_fields
                .call(&mut me.store, (stack, ptr, len))
                .map_err(|e| {
                    let trap = e
                        .downcast_ref::<wasmtime::Trap>()
                        .map(|t| format!(" [trap code: {t:?}]"))
                        .unwrap_or_default();
                    WasmError::Trap(format!("{}{trap}", e))
                })?;
            Ok(())
        })?;
        let v: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| WasmError::Wasm(format!("auth fields 非 JSON: {e}")))?;
        Ok(RuntimeAuthFields {
            encrypt_user_info: v["encrypt_user_info"].as_str().unwrap_or_default().to_string(),
            key: v["key"].as_str().unwrap_or_default().to_string(),
        })
    }

    /// 构造 QoderContext（qoder-wasm.ts:621-644）。
    pub fn context_new(
        &mut self,
        machine_id: &str,
        client_version: &str,
        user_info_json: &str,
        client_meta_json: &str,
    ) -> Result<i32, WasmError> {
        self.call_pointer(|me, stack| {
            let (m, ml) = me.write_string(machine_id)?;
            let (v, vl) = me.write_string(client_version)?;
            let (i, il) = me.write_string(user_info_json)?;
            let (meta, metal) = me.write_string(client_meta_json)?;
            me.funcs
                .context_new
                .call(&mut me.store, (stack, m, ml, v, vl, i, il, meta, metal))
                .map_err(|e| WasmError::Trap(e.to_string()))?;
            Ok(())
        })
    }

    /// 构造加密推理请求（qoder-wasm.ts:650-697）：headers 原样透传。
    pub fn prepare_infer_request(
        &mut self,
        context: i32,
        host: &str,
        payload_json: &str,
        model_key: &str,
        source: &str,
    ) -> Result<InferRequest, WasmError> {
        let result = self.call_pointer(|me, stack| {
            let (h, hl) = me.write_string(host)?;
            let (b, bl) = me.write_string(payload_json)?;
            let (k, kl) = me.write_string(model_key)?;
            let (src, srcl) = me.write_string(source)?;
            me.funcs
                .prepare_infer
                .call(&mut me.store, (stack, context, h, hl, b, bl, k, kl, src, srcl))
                .map_err(|e| WasmError::Trap(e.to_string()))?;
            Ok(())
        })?;
        let map_idx = self
            .funcs
            .result_headers
            .call(&mut self.store, result)
            .map_err(|e| WasmError::Trap(e.to_string()))?;
        let headers = match self.store.data_mut().take(map_idx as u32) {
            HostVal::Map(pairs) => pairs,
            _ => vec![],
        };
        let url = self.read_result_string(result, |f, st, stack, ptr| f.result_url.call(st, (stack, ptr)));
        let body = self.read_result_string(result, |f, st, stack, ptr| f.result_body.call(st, (stack, ptr)));
        Ok(InferRequest { url, headers, body })
    }

    /// `requestresult_url/body(stack, ptr)`——参数顺序与直觉相反（栈指针在前）。
    fn read_result_string(
        &mut self,
        result: i32,
        invoke: impl Fn(&Funcs, &mut Store<Ctx>, i32, i32) -> Result<(), wasmtime::Error>,
    ) -> String {
        let stack = self.stack_alloc(16).unwrap_or(0);
        let mut out = String::new();
        if invoke(&self.funcs, &mut self.store, stack, result).is_ok() {
            let f = self.read_frame(stack);
            if f.ptr != 0 {
                out = self.read_string(f.ptr, f.len);
            }
        }
        self.stack_free(16);
        out
    }
}

fn fill_random(buf: &mut [u8]) {
    use rand::RngCore;
    rand::rng().fill_bytes(buf);
}

// ───────── imports（31 个，签名自 wasm 二进制解析核对） ─────────

fn link_all(linker: &mut Linker<Ctx>) -> Result<(), WasmError> {
    let i32v = ValType::I32;
    macro_rules! get_i32 {
        ($args:expr, $i:literal) => {
            $args.get($i).and_then(Val::i32).unwrap_or(0)
        };
    }

    // 对象堆基础
    linker
        .func_new(IMPORT_MODULE, "__wbindgen_object_drop_ref", FuncType::new(linker.engine(), [i32v.clone()], []), |mut caller, args, _| {
            caller.data_mut().take(get_i32!(args, 0) as u32);
            Ok(())
        })
        .map_err(|e| WasmError::Abi(format!("object drop: {e}")))?;
    linker
        .func_new(IMPORT_MODULE, "__wbindgen_object_clone_ref", FuncType::new(linker.engine(), [i32v.clone()], [i32v.clone()]), |mut caller, args, results| {
            let v = caller.data().val(get_i32!(args, 0) as u32);
            let idx = caller.data_mut().push(v);
            results[0] = Val::I32(idx as i32);
            Ok(())
        })
        .map_err(|e| WasmError::Abi(format!("object clone: {e}")))?;

    // 环境探测：globalThis 携带 crypto；其余宿主对象断链
    macro_rules! push_import {
        ($name:literal, $params:expr, $results:expr, $make:expr) => {
            linker
                .func_new(IMPORT_MODULE, $name, FuncType::new(linker.engine(), $params, $results), $make)
                .map_err(|e| WasmError::Abi(format!("{}: {e}", $name)))?;
        };
    }
    push_import!("__wbg_static_accessor_GLOBAL_THIS_a1248013d790bf5f", vec![], vec![i32v.clone()], |mut caller: Caller<'_, Ctx>, _, results| {
        let idx = caller.data_mut().push(HostVal::GlobalThis);
        results[0] = Val::I32(idx as i32);
        Ok(())
    });
    push_import!("__wbg_static_accessor_SELF_24f78b6d23f286ea", vec![], vec![i32v.clone()], |mut caller: Caller<'_, Ctx>, _, results| {
        let idx = caller.data_mut().push(HostVal::Undefined);
        results[0] = Val::I32(idx as i32);
        Ok(())
    });
    push_import!("__wbg_static_accessor_GLOBAL_f2e0f995a21329ff", vec![], vec![i32v.clone()], |mut caller: Caller<'_, Ctx>, _, results| {
        // Node 的 global 也映射 globalThis（crypto 检测链会走到）
        let idx = caller.data_mut().push(HostVal::GlobalThis);
        results[0] = Val::I32(idx as i32);
        Ok(())
    });
    push_import!("__wbg_static_accessor_WINDOW_59fd959c540fe405", vec![], vec![i32v.clone()], |mut caller: Caller<'_, Ctx>, _, results| {
        let idx = caller.data_mut().push(HostVal::Undefined);
        results[0] = Val::I32(idx as i32);
        Ok(())
    });
    push_import!("__wbg_crypto_38df2bab126b63dc", vec![i32v.clone()], vec![i32v.clone()], |mut caller: Caller<'_, Ctx>, args, results| {
        let parent = caller.data().val(get_i32!(args, 0) as u32);
        let out = match parent { HostVal::GlobalThis => HostVal::Crypto, _ => HostVal::Undefined };
        let idx = caller.data_mut().push(out);
        results[0] = Val::I32(idx as i32);
        Ok(())
    });
    macro_rules! undefined_prop {
        ($name:literal) => {
            push_import!($name, vec![i32v.clone()], vec![i32v.clone()], |mut caller: Caller<'_, Ctx>, _args, results| {
                let idx = caller.data_mut().push(HostVal::Undefined);
                results[0] = Val::I32(idx as i32);
                Ok(())
            });
        };
    }
    undefined_prop!("__wbg_process_44c7a14e11e9f69e");
    undefined_prop!("__wbg_versions_276b2795b1c6a219");
    undefined_prop!("__wbg_node_84ea875411254db1");
    undefined_prop!("__wbg_msCrypto_bd5a034af96bcba6");
    push_import!("__wbg_require_b4edbdcf3e2a1ef0", vec![], vec![i32v.clone()], |mut caller: Caller<'_, Ctx>, _, results| {
        let idx = caller.data_mut().push(HostVal::Undefined);
        results[0] = Val::I32(idx as i32);
        Ok(())
    });

    // 随机：subarray 直填 + crypto.getRandomValues(视图) + randomFillSync
    push_import!("__wbg_getRandomValues_d49329ff89a07af1", vec![i32v.clone(), i32v.clone()], vec![], |mut caller: Caller<'_, Ctx>, args, _| {
        let (ptr, len) = (get_i32!(args, 0) as u32, get_i32!(args, 1).max(0) as usize);
        let mem = caller.get_export("memory").and_then(|m| m.into_memory()).ok_or(wasmtime::Error::msg("no memory"))?;
        let mut buf = vec![0u8; len];
        fill_random(&mut buf);
        mem.write(&mut caller, ptr as usize, &buf).ok();
        Ok(())
    });
    push_import!("__wbg_getRandomValues_c44a50d8cfdaebeb", vec![i32v.clone(), i32v.clone()], vec![], |mut caller: Caller<'_, Ctx>, args, _| {
        // JS 语义：crypto.getRandomValues(视图) 原位填充——索引不变
        let idx = get_i32!(args, 1) as u32;
        let mut b = match caller.data().val(idx) { HostVal::Bytes(b) => b, _ => return Ok(()) };
        fill_random(&mut b);
        caller.data_mut().heap_replace(idx, HostVal::Bytes(b));
        Ok(())
    });
    push_import!("__wbg_randomFillSync_6c25eac9869eb53c", vec![i32v.clone(), i32v.clone()], vec![], |mut caller: Caller<'_, Ctx>, args, _| {
        let idx = get_i32!(args, 1) as u32;
        if let HostVal::Bytes(mut b) = caller.data_mut().take(idx) {
            fill_random(&mut b);
            let _ = caller.data_mut().push(HostVal::Bytes(b));
        }
        Ok(())
    });

    // 字节串/数组
    push_import!("__wbg_new_with_length_9cedd08484b73942", vec![i32v.clone()], vec![i32v.clone()], |mut caller: Caller<'_, Ctx>, args, results| {
        let n = get_i32!(args, 0).max(0) as usize;
        let idx = caller.data_mut().push(HostVal::Bytes(vec![0; n]));
        results[0] = Val::I32(idx as i32);
        Ok(())
    });
    push_import!("__wbg_length_0c32cb8543c8e4c8", vec![i32v.clone()], vec![i32v.clone()], |caller: Caller<'_, Ctx>, args, results| {
        let n = match caller.data().val(get_i32!(args, 0) as u32) {
            HostVal::Bytes(b) => b.len() as i32,
            _ => 0,
        };
        results[0] = Val::I32(n);
        Ok(())
    });
    push_import!("__wbg_subarray_0f98d3fb634508ad", vec![i32v.clone(), i32v.clone(), i32v.clone()], vec![i32v.clone()], |mut caller: Caller<'_, Ctx>, args, results| {
        let (a, s, e) = (get_i32!(args, 0) as u32, get_i32!(args, 1).max(0) as usize, get_i32!(args, 2).max(0) as usize);
        let out = match caller.data().val(a) {
            HostVal::Bytes(b) => {
                // clamp 到 [0, len] 且保证 start<=end：畸形 (s>e) 会切出反向区间 panic
                let end = e.min(b.len());
                let start = s.min(end);
                HostVal::Bytes(b[start..end].to_vec())
            }
            _ => HostVal::Bytes(vec![]),
        };
        let idx = caller.data_mut().push(out);
        results[0] = Val::I32(idx as i32);
        Ok(())
    });
    push_import!("__wbg_set_08463b1df38a7e29", vec![i32v.clone(), i32v.clone(), i32v.clone()], vec![i32v.clone()], |mut caller: Caller<'_, Ctx>, args, results| {
        let (dst, src, off) = (get_i32!(args, 0) as u32, get_i32!(args, 1) as u32, get_i32!(args, 2).max(0) as usize);
        let s = match caller.data().val(src) { HostVal::Bytes(b) => b, _ => vec![] };
        let out = match caller.data_mut().take(dst) {
            HostVal::Bytes(mut d) => {
                let end = (off + s.len()).min(d.len());
                if off < d.len() && end > off {
                    d[off..end].copy_from_slice(&s[..end - off]);
                }
                HostVal::Bytes(d)
            }
            _ => HostVal::Bytes(s),
        };
        let idx = caller.data_mut().push(out);
        results[0] = Val::I32(idx as i32);
        Ok(())
    });

    // Map / now / call / Error / throw / 判型
    push_import!("__wbg_new_99cabae501c0a8a0", vec![], vec![i32v.clone()], |mut caller: Caller<'_, Ctx>, _, results| {
        let idx = caller.data_mut().push(HostVal::Map(vec![]));
        results[0] = Val::I32(idx as i32);
        Ok(())
    });
    push_import!("__wbg_prototypesetcall_3e05eb9545565046", vec![i32v.clone(), i32v.clone(), i32v.clone()], vec![], |mut caller: Caller<'_, Ctx>, args, _| {
        // Uint8Array.prototype.set.call(heap().subarray(a, a+e), 堆数组 t)：
        // 把 wasm 内存 a..a+e 的字节 set 进堆数组 t（从 0 开始）
        let (a, e, t) = (get_i32!(args, 0) as u32, get_i32!(args, 1).max(0) as usize, get_i32!(args, 2) as u32);
        let mem = caller.get_export("memory").and_then(|x| x.into_memory()).ok_or(wasmtime::Error::msg("no memory"))?;
        let mut buf = vec![0u8; e];
        mem.read(&mut caller, a as usize, &mut buf).ok();
        let mut d = match caller.data().val(t) { HostVal::Bytes(b) => b, _ => vec![] };
        let n = buf.len().min(d.len());
        d[..n].copy_from_slice(&buf[..n]);
        caller.data_mut().heap_replace(t, HostVal::Bytes(d));
        Ok(())
    });
    push_import!("__wbg_now_88621c9c9a4f3ffc", vec![], vec![ValType::F64], |_caller: Caller<'_, Ctx>, _, results| {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0);
        results[0] = Val::F64(ms.to_bits());
        Ok(())
    });
    push_import!("__wbg_call_d578befcc3145dee", vec![i32v.clone(), i32v.clone(), i32v.clone()], vec![i32v.clone()], |mut caller: Caller<'_, Ctx>, _, results| {
        // 本宿主不提供 JS Function 对象；headers 构建不走此路径。
        let idx = caller.data_mut().push(HostVal::Undefined);
        results[0] = Val::I32(idx as i32);
        Ok(())
    });
    push_import!("__wbg_Error_2e59b1b37a9a34c3", vec![i32v.clone(), i32v.clone()], vec![i32v.clone()], |mut caller: Caller<'_, Ctx>, args, results| {
        let mem = caller.get_export("memory").and_then(|m| m.into_memory()).ok_or(wasmtime::Error::msg("no memory"))?;
        let mut buf = vec![0u8; get_i32!(args, 1).max(0) as usize];
        mem.read(&mut caller, get_i32!(args, 0) as u32 as usize, &mut buf).ok();
        let idx = caller.data_mut().push(HostVal::Str(String::from_utf8_lossy(&buf).to_string()));
        results[0] = Val::I32(idx as i32);
        Ok(())
    });
    push_import!("__wbg___wbindgen_throw_81fc77679af83bc6", vec![i32v.clone(), i32v.clone()], vec![], |mut caller: Caller<'_, Ctx>, args, _| {
        let mem = caller.get_export("memory").and_then(|m| m.into_memory()).ok_or(wasmtime::Error::msg("no memory"))?;
        let mut buf = vec![0u8; get_i32!(args, 1).max(0) as usize];
        mem.read(&mut caller, get_i32!(args, 0) as u32 as usize, &mut buf).ok();
        let msg = String::from_utf8_lossy(&buf).to_string();
        Err(wasmtime::Error::msg(format!("wasm throw: {msg}")))
    });
    push_import!("__wbg___wbindgen_is_object_40c5a80572e8f9d3", vec![i32v.clone()], vec![i32v.clone()], |caller: Caller<'_, Ctx>, args, results| {
        let v = caller.data().val(get_i32!(args, 0) as u32);
        results[0] = Val::I32(v.is_object() as i32);
        Ok(())
    });
    push_import!("__wbg___wbindgen_is_string_b29b5c5a8065ba1a", vec![i32v.clone()], vec![i32v.clone()], |caller: Caller<'_, Ctx>, args, results| {
        let v = caller.data().val(get_i32!(args, 0) as u32);
        results[0] = Val::I32(v.is_string() as i32);
        Ok(())
    });
    push_import!("__wbg___wbindgen_is_function_49868bde5eb1e745", vec![i32v.clone()], vec![i32v.clone()], |caller: Caller<'_, Ctx>, args, results| {
        let idx = get_i32!(args, 0) as u32;
        let is_fn = matches!(caller.data().val(idx), HostVal::Crypto | HostVal::GlobalThis);
        results[0] = Val::I32(is_fn as i32);
        Ok(())
    });
    push_import!("__wbg___wbindgen_is_undefined_c0cca72b82b86f4d", vec![i32v.clone()], vec![i32v.clone()], |caller: Caller<'_, Ctx>, args, results| {
        let v = caller.data().val(get_i32!(args, 0) as u32);
        results[0] = Val::I32(v.is_undefined() as i32);
        Ok(())
    });

    // cast：内存 → JS 值
    push_import!("__wbindgen_cast_0000000000000001", vec![i32v.clone(), i32v.clone()], vec![i32v.clone()], |mut caller: Caller<'_, Ctx>, args, results| {
        let mem = caller.get_export("memory").and_then(|m| m.into_memory()).ok_or(wasmtime::Error::msg("no memory"))?;
        let mut buf = vec![0u8; get_i32!(args, 1).max(0) as usize];
        mem.read(&mut caller, get_i32!(args, 0) as u32 as usize, &mut buf).ok();
        let idx = caller.data_mut().push(HostVal::Bytes(buf));
        results[0] = Val::I32(idx as i32);
        Ok(())
    });
    push_import!("__wbindgen_cast_0000000000000002", vec![i32v.clone(), i32v.clone()], vec![i32v.clone()], |mut caller: Caller<'_, Ctx>, args, results| {
        let mem = caller.get_export("memory").and_then(|m| m.into_memory()).ok_or(wasmtime::Error::msg("no memory"))?;
        let mut buf = vec![0u8; get_i32!(args, 1).max(0) as usize];
        mem.read(&mut caller, get_i32!(args, 0) as u32 as usize, &mut buf).ok();
        let idx = caller.data_mut().push(HostVal::Str(String::from_utf8_lossy(&buf).to_string()));
        results[0] = Val::I32(idx as i32);
        Ok(())
    });
    Ok(())
}
