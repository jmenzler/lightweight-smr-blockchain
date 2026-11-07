let wasm_bindgen = (function(exports) {
    let script_src;
    if (typeof document !== 'undefined' && document.currentScript !== null) {
        script_src = new URL(document.currentScript.src, location.href).toString();
    }

    /**
     * Stepwise simulator session whose recorded history replays byte-identical via `run_scenario_json`.
     * The blocking fraction is chosen interactively each round, so a session is not a 1-late adversary run.
     */
    class LiveSim {
        static __wrap(ptr) {
            const obj = Object.create(LiveSim.prototype);
            obj.__wbg_ptr = ptr;
            LiveSimFinalization.register(obj, obj.__wbg_ptr, obj);
            return obj;
        }
        __destroy_into_raw() {
            const ptr = this.__wbg_ptr;
            this.__wbg_ptr = 0;
            LiveSimFinalization.unregister(this);
            return ptr;
        }
        free() {
            const ptr = this.__destroy_into_raw();
            wasm.__wbg_livesim_free(ptr, 0);
        }
        /**
         * @returns {string}
         */
        export_scenario() {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.livesim_export_scenario(this.__wbg_ptr);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
        /**
         * Spec's `schedule` is ignored: no up-front mask draw may touch the RNG stream.
         * @param {string} spec_json
         */
        constructor(spec_json) {
            const ptr0 = passStringToWasm0(spec_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.livesim_new(ptr0, len0);
            if (ret[2]) {
                throw takeFromExternrefTable0(ret[1]);
            }
            this.__wbg_ptr = ret[0];
            LiveSimFinalization.register(this, this.__wbg_ptr, this);
            return this;
        }
        /**
         * `sticky`: the blocked set persists between steps, adjusted by delta-sampling.
         * @param {string} spec_json
         * @param {boolean} sticky
         * @returns {LiveSim}
         */
        static new_with_mode(spec_json, sticky) {
            const ptr0 = passStringToWasm0(spec_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.livesim_new_with_mode(ptr0, len0, sticky);
            if (ret[2]) {
                throw takeFromExternrefTable0(ret[1]);
            }
            return LiveSim.__wrap(ret[0]);
        }
        /**
         * Advance one round at `fraction` blocking (an error JSON outside [0, 1]); no-op once absorbed so the replay length matches.
         * @param {number} fraction
         * @returns {string}
         */
        step(fraction) {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.livesim_step(this.__wbg_ptr, fraction);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
        /**
         * @returns {string}
         */
        trace_json() {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.livesim_trace_json(this.__wbg_ptr);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
    }
    if (Symbol.dispose) LiveSim.prototype[Symbol.dispose] = LiveSim.prototype.free;
    exports.LiveSim = LiveSim;

    /**
     * Stepwise SMR session whose history and injections replay byte-identically via `run_smr_json`.
     */
    class LiveSmr {
        static __wrap(ptr) {
            const obj = Object.create(LiveSmr.prototype);
            obj.__wbg_ptr = ptr;
            LiveSmrFinalization.register(obj, obj.__wbg_ptr, obj);
            return obj;
        }
        __destroy_into_raw() {
            const ptr = this.__wbg_ptr;
            this.__wbg_ptr = 0;
            LiveSmrFinalization.unregister(this);
            return ptr;
        }
        free() {
            const ptr = this.__destroy_into_raw();
            wasm.__wbg_livesmr_free(ptr, 0);
        }
        /**
         * Freeze the client's current bare-newest certificate for the §5 staleness demo.
         * @param {number} client
         * @returns {string}
         */
        capture_stale(client) {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.livesmr_capture_stale(this.__wbg_ptr, client);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
        /**
         * Forest + certificate snapshot for the live drawer (pure read).
         * @returns {string}
         */
        certs_json() {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.livesmr_certs_json(this.__wbg_ptr);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
        /**
         * The longest executed sequence over the n servers (a display construct, not any one server's S_i)
         * as a Merkle forest snapshot, delta-encoded from `from_len`.
         * @param {bigint} from_len
         * @returns {string}
         */
        chain_json(from_len) {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.livesmr_chain_json(this.__wbg_ptr, from_len);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
        /**
         * @returns {string}
         */
        export_scenario() {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.livesmr_export_scenario(this.__wbg_ptr);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
        /**
         * Submit a command live (lands next round); a pinned `target` deviates from the paper's random-server client.
         * @param {number} client
         * @param {bigint} op
         * @param {number | null} [target]
         * @returns {string}
         */
        inject(client, op, target) {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.livesmr_inject(this.__wbg_ptr, client, op, isLikeNone(target) ? Number.MAX_SAFE_INTEGER : (target) >>> 0);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
        /**
         * Every node hash of the forest tree covering positions [start, start + 2^height), recomputed
         * from the same committed sequence the drawer's forest is built over (certs: the drawer's best
         * server; otherwise the committed chain). The caller checks the top against the peak root.
         * @param {bigint} start
         * @param {number} height
         * @returns {string}
         */
        merkle_tree_json(start, height) {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.livesmr_merkle_tree_json(this.__wbg_ptr, start, height);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
        /**
         * Spec's `schedule` is ignored: no up-front mask draw may touch the RNG stream.
         * @param {string} spec_json
         */
        constructor(spec_json) {
            const ptr0 = passStringToWasm0(spec_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.livesmr_new(ptr0, len0);
            if (ret[2]) {
                throw takeFromExternrefTable0(ret[1]);
            }
            this.__wbg_ptr = ret[0];
            LiveSmrFinalization.register(this, this.__wbg_ptr, this);
            return this;
        }
        /**
         * Same sticky semantics as `LiveSim::new_with_mode`.
         * @param {string} spec_json
         * @param {boolean} sticky
         * @returns {LiveSmr}
         */
        static new_with_mode(spec_json, sticky) {
            const ptr0 = passStringToWasm0(spec_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.livesmr_new_with_mode(ptr0, len0, sticky);
            if (ret[2]) {
                throw takeFromExternrefTable0(ret[1]);
            }
            return LiveSmr.__wrap(ret[0]);
        }
        /**
         * @param {number} node
         * @returns {string}
         */
        node_detail(node) {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.livesmr_node_detail(this.__wbg_ptr, node);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
        /**
         * What changed in the report since the previous call (the first call: since nothing), so a
         * per-step message does not grow with the run. The page rebuilds `report_json()` from it.
         * @returns {string}
         */
        report_delta_json() {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.livesmr_report_delta_json(this.__wbg_ptr);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
        /**
         * The full report so far (metrics, per-command audit trails, terminal).
         * @returns {string}
         */
        report_json() {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.livesmr_report_json(this.__wbg_ptr);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
        /**
         * Block or release one node by hand; lands next round so the export still replays.
         * @param {number} node
         * @param {boolean} blocked
         * @returns {string}
         */
        set_block(node, blocked) {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.livesmr_set_block(this.__wbg_ptr, node, blocked);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
        /**
         * Live arrival-pmf edit; applies from the next round and replaces pending and future phases.
         * @param {string} pmf_json
         * @returns {string}
         */
        set_traffic(pmf_json) {
            let deferred2_0;
            let deferred2_1;
            try {
                const ptr0 = passStringToWasm0(pmf_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
                const len0 = WASM_VECTOR_LEN;
                const ret = wasm.livesmr_set_traffic(this.__wbg_ptr, ptr0, len0);
                deferred2_0 = ret[0];
                deferred2_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
            }
        }
        /**
         * Advance one round at `fraction` blocking (an error JSON outside [0, 1]); no-op once dead so the replay length matches.
         * @param {number} fraction
         * @returns {string}
         */
        step(fraction) {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.livesmr_step(this.__wbg_ptr, fraction);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
        /**
         * On-demand verify-everywhere tallies for every issued certificate of every client.
         * @returns {string}
         */
        verify_json() {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.livesmr_verify_json(this.__wbg_ptr);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
    }
    if (Symbol.dispose) LiveSmr.prototype[Symbol.dispose] = LiveSmr.prototype.free;
    exports.LiveSmr = LiveSmr;

    /**
     * A parsed canonical spill: the lab's view of a lean recovery run.
     */
    class SpillView {
        __destroy_into_raw() {
            const ptr = this.__wbg_ptr;
            this.__wbg_ptr = 0;
            SpillViewFinalization.unregister(this);
            return ptr;
        }
        free() {
            const ptr = this.__destroy_into_raw();
            wasm.__wbg_spillview_free(ptr, 0);
        }
        /**
         * The recovery chain forest over one node's reconstructed view at one boundary.
         * @param {number} boundary
         * @param {number} node
         * @param {bigint} from_len
         * @returns {string}
         */
        chain_json(boundary, node, from_len) {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.spillview_chain_json(this.__wbg_ptr, boundary, node, from_len);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
        /**
         * @param {string} text
         */
        constructor(text) {
            const ptr0 = passStringToWasm0(text, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.spillview_new(ptr0, len0);
            if (ret[2]) {
                throw takeFromExternrefTable0(ret[1]);
            }
            this.__wbg_ptr = ret[0];
            SpillViewFinalization.register(this, this.__wbg_ptr, this);
            return this;
        }
        /**
         * Provenance, per-boundary layout, and verification status for the viewer.
         * @returns {string}
         */
        summary_json() {
            let deferred1_0;
            let deferred1_1;
            try {
                const ret = wasm.spillview_summary_json(this.__wbg_ptr);
                deferred1_0 = ret[0];
                deferred1_1 = ret[1];
                return getStringFromWasm0(ret[0], ret[1]);
            } finally {
                wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
            }
        }
    }
    if (Symbol.dispose) SpillView.prototype[Symbol.dispose] = SpillView.prototype.free;
    exports.SpillView = SpillView;

    /**
     * Records every panic message before the wasm trap, chaining to the previous hook.
     */
    function install_panic_hook() {
        wasm.install_panic_hook();
    }
    exports.install_panic_hook = install_panic_hook;

    /**
     * The last panic this thread saw; the engine state behind it is unusable.
     * @returns {string | undefined}
     */
    function last_panic() {
        const ret = wasm.last_panic();
        let v1;
        if (ret[0] !== 0) {
            v1 = getStringFromWasm0(ret[0], ret[1]).slice();
            wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        }
        return v1;
    }
    exports.last_panic = last_panic;

    /**
     * Run a full scenario from spec JSON; returns `{trace, report}` or `{error}` JSON.
     * @param {string} spec_json
     * @returns {string}
     */
    function run_scenario_json(spec_json) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ptr0 = passStringToWasm0(spec_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.run_scenario_json(ptr0, len0);
            deferred2_0 = ret[0];
            deferred2_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    exports.run_scenario_json = run_scenario_json;

    /**
     * Run a full SMR scenario from spec JSON; returns `{report}` or `{error}` JSON.
     * @param {string} spec_json
     * @returns {string}
     */
    function run_smr_json(spec_json) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ptr0 = passStringToWasm0(spec_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.run_smr_json(ptr0, len0);
            deferred2_0 = ret[0];
            deferred2_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    exports.run_smr_json = run_smr_json;
    function __wbg_get_imports() {
        const import0 = {
            __proto__: null,
            __wbg___wbindgen_throw_344f42d3211c4765: function(arg0, arg1) {
                throw new Error(getStringFromWasm0(arg0, arg1));
            },
            __wbindgen_cast_0000000000000001: function(arg0, arg1) {
                // Cast intrinsic for `Ref(String) -> Externref`.
                const ret = getStringFromWasm0(arg0, arg1);
                return ret;
            },
            __wbindgen_init_externref_table: function() {
                const table = wasm.__wbindgen_externrefs;
                const offset = table.grow(4);
                table.set(0, undefined);
                table.set(offset + 0, undefined);
                table.set(offset + 1, null);
                table.set(offset + 2, true);
                table.set(offset + 3, false);
            },
        };
        return {
            __proto__: null,
            "./sim_wasm_bg.js": import0,
        };
    }

    const LiveSimFinalization = (typeof FinalizationRegistry === 'undefined')
        ? { register: () => {}, unregister: () => {} }
        : new FinalizationRegistry(ptr => wasm.__wbg_livesim_free(ptr, 1));
    const LiveSmrFinalization = (typeof FinalizationRegistry === 'undefined')
        ? { register: () => {}, unregister: () => {} }
        : new FinalizationRegistry(ptr => wasm.__wbg_livesmr_free(ptr, 1));
    const SpillViewFinalization = (typeof FinalizationRegistry === 'undefined')
        ? { register: () => {}, unregister: () => {} }
        : new FinalizationRegistry(ptr => wasm.__wbg_spillview_free(ptr, 1));

    function getStringFromWasm0(ptr, len) {
        return decodeText(ptr >>> 0, len);
    }

    let cachedUint8ArrayMemory0 = null;
    function getUint8ArrayMemory0() {
        if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
            cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
        }
        return cachedUint8ArrayMemory0;
    }

    function isLikeNone(x) {
        return x === undefined || x === null;
    }

    function passStringToWasm0(arg, malloc, realloc) {
        if (realloc === undefined) {
            const buf = cachedTextEncoder.encode(arg);
            const ptr = malloc(buf.length, 1) >>> 0;
            getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
            WASM_VECTOR_LEN = buf.length;
            return ptr;
        }

        let len = arg.length;
        let ptr = malloc(len, 1) >>> 0;

        const mem = getUint8ArrayMemory0();

        let offset = 0;

        for (; offset < len; offset++) {
            const code = arg.charCodeAt(offset);
            if (code > 0x7F) break;
            mem[ptr + offset] = code;
        }
        if (offset !== len) {
            if (offset !== 0) {
                arg = arg.slice(offset);
            }
            ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
            const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
            const ret = cachedTextEncoder.encodeInto(arg, view);

            offset += ret.written;
            ptr = realloc(ptr, len, offset, 1) >>> 0;
        }

        WASM_VECTOR_LEN = offset;
        return ptr;
    }

    function takeFromExternrefTable0(idx) {
        const value = wasm.__wbindgen_externrefs.get(idx);
        wasm.__externref_table_dealloc(idx);
        return value;
    }

    let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
    cachedTextDecoder.decode();
    function decodeText(ptr, len) {
        return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
    }

    const cachedTextEncoder = new TextEncoder();

    if (!('encodeInto' in cachedTextEncoder)) {
        cachedTextEncoder.encodeInto = function (arg, view) {
            const buf = cachedTextEncoder.encode(arg);
            view.set(buf);
            return {
                read: arg.length,
                written: buf.length
            };
        };
    }

    let WASM_VECTOR_LEN = 0;

    let wasmModule, wasmInstance, wasm;
    function __wbg_finalize_init(instance, module) {
        wasmInstance = instance;
        wasm = instance.exports;
        wasmModule = module;
        cachedUint8ArrayMemory0 = null;
        wasm.__wbindgen_start();
        return wasm;
    }

    async function __wbg_load(module, imports) {
        if (typeof Response === 'function' && module instanceof Response) {
            if (typeof WebAssembly.instantiateStreaming === 'function') {
                try {
                    return await WebAssembly.instantiateStreaming(module, imports);
                } catch (e) {
                    const validResponse = module.ok && expectedResponseType(module.type);

                    if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                        console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                    } else { throw e; }
                }
            }

            const bytes = await module.arrayBuffer();
            return await WebAssembly.instantiate(bytes, imports);
        } else {
            const instance = await WebAssembly.instantiate(module, imports);

            if (instance instanceof WebAssembly.Instance) {
                return { instance, module };
            } else {
                return instance;
            }
        }

        function expectedResponseType(type) {
            switch (type) {
                case 'basic': case 'cors': case 'default': return true;
            }
            return false;
        }
    }

    function initSync(module) {
        if (wasm !== undefined) return wasm;


        if (module !== undefined) {
            if (Object.getPrototypeOf(module) === Object.prototype) {
                ({module} = module)
            } else {
                console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
            }
        }

        const imports = __wbg_get_imports();
        if (!(module instanceof WebAssembly.Module)) {
            module = new WebAssembly.Module(module);
        }
        const instance = new WebAssembly.Instance(module, imports);
        return __wbg_finalize_init(instance, module);
    }

    async function __wbg_init(module_or_path) {
        if (wasm !== undefined) return wasm;


        if (module_or_path !== undefined) {
            if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
                ({module_or_path} = module_or_path)
            } else {
                console.warn('using deprecated parameters for the initialization function; pass a single object instead')
            }
        }

        if (module_or_path === undefined && script_src !== undefined) {
            module_or_path = script_src.replace(/\.js$/, "_bg.wasm");
        }
        const imports = __wbg_get_imports();

        if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
            module_or_path = fetch(module_or_path);
        }

        const { instance, module } = await __wbg_load(await module_or_path, imports);

        return __wbg_finalize_init(instance, module);
    }

    return Object.assign(__wbg_init, { initSync }, exports);
})({ __proto__: null });
