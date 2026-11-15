import CLua
import Foundation

/// An error raised in or for Lua, with the message Lua would show.
struct LuaError: Error, CustomStringConvertible, LocalizedError {
    var description: String
    init(_ description: String) { self.description = description }
    var errorDescription: String? { description }
}

/// A Lua value kept alive in the registry, such as an action's `run`
/// function. Released on the VM's queue when the last Swift copy goes.
final class LuaRef: @unchecked Sendable {
    let id: Int32
    private weak var vm: LuaVM?

    fileprivate init(id: Int32, vm: LuaVM) {
        self.id = id
        self.vm = vm
    }

    deinit {
        guard let vm else { return }
        let id = self.id
        vm.queue.async { [weak vm] in vm?.release(id) }
    }
}

/// One Lua state, used only on `queue`. Swift functions are exposed through
/// `Lua.pushFunction`; Lua errors never unwind through Swift.
final class LuaVM: @unchecked Sendable {
    let state: OpaquePointer
    let queue: DispatchQueue
    private var boxes: [Unmanaged<FunctionBox>] = []
    private(set) var isClosed = false

    final class FunctionBox {
        let body: (Lua) throws -> Int32
        unowned let vm: LuaVM
        init(vm: LuaVM, body: @escaping (Lua) throws -> Int32) {
            self.vm = vm
            self.body = body
        }
    }

    private static let installCallback: Void = {
        clua_set_swift_callback { box, state in
            guard let box, let state else { return 0 }
            let function = Unmanaged<FunctionBox>.fromOpaque(box).takeUnretainedValue()
            let lua = Lua(state: state, vm: function.vm)
            do {
                return try function.body(lua)
            } catch {
                lua.push((error as? LuaError)?.description ?? "\(error)")
                return -1
            }
        }
    }()

    init(queue: DispatchQueue) {
        _ = Self.installCallback
        self.queue = queue
        state = luaL_newstate()
        clua_openlibs(state)
    }

    deinit { close() }

    func close() {
        guard !isClosed else { return }
        isClosed = true
        lua_close(state)
        boxes.forEach { $0.release() }
        boxes.removeAll()
    }

    var main: Lua { Lua(state: state, vm: self) }

    fileprivate func retain(_ box: FunctionBox) -> UnsafeMutableRawPointer {
        let unmanaged = Unmanaged.passRetained(box)
        boxes.append(unmanaged)
        return unmanaged.toOpaque()
    }

    fileprivate func makeRef(_ id: Int32) -> LuaRef { LuaRef(id: id, vm: self) }

    func release(_ id: Int32) {
        guard !isClosed else { return }
        luaL_unref(state, clua_registryindex(), id)
    }

    /// Loads and runs `code`, returning what it returns.
    @discardableResult
    func run(_ code: String, name: String) throws -> [Any?] {
        try main.run(code, name: name)
    }

    /// Calls `function` with `arguments`, returning its results.
    @discardableResult
    func call(_ function: LuaRef, _ arguments: [Any?] = []) throws -> [Any?] {
        try main.call(function, arguments)
    }
}

/// A view of one Lua thread's stack: the main state, or the coroutine that
/// called a Swift function.
struct Lua {
    let state: OpaquePointer
    unowned let vm: LuaVM

    var top: Int32 { lua_gettop(state) }
    func setTop(_ n: Int32) { lua_settop(state, n) }
    func pop(_ n: Int32 = 1) { clua_pop(state, n) }
    func type(_ index: Int32) -> Int32 { lua_type(state, index) }
    func isNil(_ index: Int32) -> Bool { type(index) <= LUA_TNIL }
    func typeName(_ index: Int32) -> String { String(cString: lua_typename(state, type(index))) }

    // MARK: Reading

    func string(_ index: Int32) -> String? {
        guard type(index) == LUA_TSTRING || type(index) == LUA_TNUMBER else { return nil }
        // Copy a number first: tolstring would turn it into a string in place.
        lua_pushvalue(state, index)
        defer { pop() }
        var length = 0
        guard let pointer = clua_tostring(state, -1, &length) else { return nil }
        return String(decoding: UnsafeRawBufferPointer(start: pointer, count: length), as: UTF8.self)
    }

    func number(_ index: Int32) -> Double? {
        type(index) == LUA_TNUMBER ? clua_tonumber(state, index) : nil
    }

    func bool(_ index: Int32) -> Bool { lua_toboolean(state, index) != 0 }

    /// A reference to the value at `index`, which stays alive while the ref does.
    func ref(_ index: Int32) -> LuaRef {
        lua_pushvalue(state, index)
        return vm.makeRef(luaL_ref(state, clua_registryindex()))
    }

    /// The value at `index` as Swift: nil, Bool, Int, Double, String,
    /// [Any] for a sequence, [String: Any] for other tables, LuaRef for
    /// functions and userdata.
    func value(_ index: Int32, depth: Int = 0) -> Any? {
        let index = lua_absindex(state, index)
        switch type(index) {
        case LUA_TNIL, LUA_TNONE: return nil
        case LUA_TBOOLEAN: return bool(index)
        case LUA_TNUMBER:
            if lua_isinteger(state, index) != 0 { return Int(clua_tointeger(state, index, nil)) }
            return clua_tonumber(state, index)
        case LUA_TSTRING: return string(index)
        case LUA_TTABLE:
            guard depth < 64 else { return nil }
            return table(index, depth: depth)
        default: return ref(index)
        }
    }

    private func table(_ index: Int32, depth: Int) -> Any {
        var dict: [String: Any] = [:]
        var array: [Int: Any] = [:]
        var isArray = true
        lua_pushnil(state)
        while lua_next(state, index) != 0 {
            // key at -2, value at -1
            let value = self.value(-1, depth: depth + 1)
            if type(-2) == LUA_TNUMBER, lua_isinteger(state, -2) != 0 {
                let key = Int(clua_tointeger(state, -2, nil))
                if let value { array[key] = value }
                dict[String(key)] = value
            } else {
                isArray = false
                if let key = string(-2), let value { dict[key] = value }
            }
            pop()
        }
        if isArray, !array.isEmpty, array.keys.min() == 1, array.keys.max() == array.count {
            return (1...array.count).map { array[$0]! }
        }
        return dict
    }

    func field(_ index: Int32, _ name: String) -> Any? {
        let index = lua_absindex(state, index)
        guard type(index) == LUA_TTABLE else { return nil }
        lua_pushstring(state, name)
        lua_rawget(state, index)
        defer { pop() }
        return value(-1)
    }

    // MARK: Writing

    func push(_ value: Any?) {
        switch value {
        case nil, is NSNull:
            lua_pushnil(state)
        case let string as String:
            pushString(string)
        case let substring as Substring:
            pushString(String(substring))
        case let ref as LuaRef:
            lua_rawgeti(state, clua_registryindex(), lua_Integer(ref.id))
        case let number as NSNumber:
            if CFGetTypeID(number) == CFBooleanGetTypeID() {
                lua_pushboolean(state, number.boolValue ? 1 : 0)
            } else if CFNumberIsFloatType(number) {
                lua_pushnumber(state, number.doubleValue)
            } else {
                lua_pushinteger(state, lua_Integer(number.int64Value))
            }
        case let array as [Any?]:
            lua_createtable(state, Int32(array.count), 0)
            for (i, element) in array.enumerated() {
                push(element)
                lua_rawseti(state, -2, lua_Integer(i + 1))
            }
        case let array as [Any]:
            push(array.map { Optional($0) })
        case let dict as [String: Any?]:
            lua_createtable(state, 0, Int32(dict.count))
            for (key, element) in dict {
                guard let element else { continue }
                pushString(key)
                push(element)
                lua_rawset(state, -3)
            }
        case let dict as [String: Any]:
            push(dict.mapValues { Optional($0) })
        case let function as LuaFunction:
            pushFunction(function.body)
        default:
            pushString("\(value!)")
        }
    }

    func pushString(_ string: String) {
        var copy = string
        copy.withUTF8 { buffer in
            buffer.withMemoryRebound(to: CChar.self) { chars in
                _ = lua_pushlstring(state, chars.baseAddress, chars.count)
            }
        }
    }

    func pushFunction(_ body: @escaping (Lua) throws -> Int32) {
        let box = LuaVM.FunctionBox(vm: vm, body: body)
        clua_pushswift(state, vm.retain(box))
    }

    /// Sets `table[name] = value` for the table at `index`.
    func setField(_ index: Int32, _ name: String, _ value: Any?) {
        let index = lua_absindex(state, index)
        pushString(name)
        push(value)
        lua_rawset(state, index)
    }

    func setFunction(_ index: Int32, _ name: String, _ body: @escaping (Lua) throws -> Int32) {
        let index = lua_absindex(state, index)
        pushString(name)
        pushFunction(body)
        lua_rawset(state, index)
    }

    func setGlobal(_ name: String) { lua_setglobal(state, name) }

    func getGlobal(_ name: String) { lua_getglobal(state, name) }

    // MARK: Calling

    func run(_ code: String, name: String) throws -> [Any?] {
        let base = top
        var copy = code
        let status = copy.withUTF8 { buffer in
            buffer.withMemoryRebound(to: CChar.self) { chars in
                clua_loadbuffer(state, chars.baseAddress, chars.count, name)
            }
        }
        guard status == LUA_OK else {
            let message = string(-1) ?? "can't load \(name)"
            setTop(base)
            throw LuaError(message)
        }
        return try pcall(arguments: 0, base: base)
    }

    func call(_ function: LuaRef, _ arguments: [Any?]) throws -> [Any?] {
        let base = top
        push(function)
        for argument in arguments { push(argument) }
        return try pcall(arguments: Int32(arguments.count), base: base)
    }

    /// Calls the function below `arguments` values, leaving the stack at `base`.
    private func pcall(arguments: Int32, base: Int32) throws -> [Any?] {
        let status = clua_pcall(state, arguments, LUA_MULTRET, 0)
        defer { setTop(base) }
        guard status == LUA_OK else {
            throw LuaError(errorMessage(-1))
        }
        let count = top - base
        return count > 0 ? (1...count).map { value(base + $0) } : []
    }

    /// What an error value says: its text, or its `message` field, or its type.
    func errorMessage(_ index: Int32) -> String {
        if let text = string(index) { return text }
        if type(index) == LUA_TTABLE, let message = field(index, "message") as? String { return message }
        return "(error object is a \(typeName(index)) value)"
    }

    /// Raises `message` as a Lua error from a Swift function.
    func error(_ message: String) -> LuaError { LuaError(message) }
}

/// A Swift function to hand Lua as a value, such as a method in a returned table.
struct LuaFunction {
    let body: (Lua) throws -> Int32
    init(_ body: @escaping (Lua) throws -> Int32) { self.body = body }
}
