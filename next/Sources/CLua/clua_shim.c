#include "CLua.h"

static clua_swift_fn swift_callback = 0;

void clua_set_swift_callback(clua_swift_fn fn) { swift_callback = fn; }

static int clua_trampoline(lua_State *L) {
    void *box = lua_touserdata(L, lua_upvalueindex(1));
    int n = swift_callback ? swift_callback(box, L) : 0;
    if (n < 0) {
        // Name the Lua line that called, as luaL_error does: "init.lua:3: …".
        if (lua_type(L, -1) == LUA_TSTRING) {
            luaL_where(L, 1);
            lua_insert(L, -2);
            lua_concat(L, 2);
        }
        return lua_error(L);  // The Swift frame is gone; raising is safe here.
    }
    return n;
}

void clua_pushswift(lua_State *L, void *box) {
    lua_pushlightuserdata(L, box);
    lua_pushcclosure(L, clua_trampoline, 1);
}

int clua_loadbuffer(lua_State *L, const char *chunk, size_t len, const char *name) {
    return luaL_loadbufferx(L, chunk, len, name, "t");
}

int clua_traceback(lua_State *L) {
    const char *msg = lua_tostring(L, 1);
    if (msg == NULL) {
        if (luaL_callmeta(L, 1, "__tostring") && lua_type(L, -1) == LUA_TSTRING) return 1;
        msg = lua_pushfstring(L, "(error object is a %s value)", luaL_typename(L, 1));
    }
    luaL_traceback(L, L, msg, 1);
    return 1;
}

void clua_openlibs(lua_State *L) { luaL_openlibs(L); }

int clua_caller(lua_State *L, int level, const char **source, int *line) {
    lua_Debug ar;
    if (!lua_getstack(L, level, &ar)) return 0;
    if (!lua_getinfo(L, "Sl", &ar)) return 0;
    *source = ar.source;
    *line = ar.currentline;
    return 1;
}
