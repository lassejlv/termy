#ifndef CLUA_H
#define CLUA_H

#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

// Lua's API is partly macros, which Swift can't call, and raises errors with
// longjmp, which must never cross Swift frames. These wrappers cover both:
// Swift functions are called through `clua_pushswift`'s trampoline, which
// raises the error itself once the Swift frame has returned.

/// Called for every Swift function: `box` is what `clua_pushswift` was given.
/// Returns the number of results, or -1 with an error value on the stack top.
typedef int (*clua_swift_fn)(void *box, lua_State *L);

void clua_set_swift_callback(clua_swift_fn fn);
/// Pushes a Lua function that calls the Swift callback with `box`.
void clua_pushswift(lua_State *L, void *box);

static inline int clua_upvalueindex(int i) { return lua_upvalueindex(i); }
static inline int clua_registryindex(void) { return LUA_REGISTRYINDEX; }
static inline void clua_pop(lua_State *L, int n) { lua_pop(L, n); }
static inline void clua_newtable(lua_State *L) { lua_newtable(L); }
static inline int clua_pcall(lua_State *L, int nargs, int nresults, int errfunc) {
    return lua_pcall(L, nargs, nresults, errfunc);
}
static inline const char *clua_tostring(lua_State *L, int idx, size_t *len) { return lua_tolstring(L, idx, len); }
static inline lua_Number clua_tonumber(lua_State *L, int idx) { return lua_tonumberx(L, idx, NULL); }
static inline lua_Integer clua_tointeger(lua_State *L, int idx, int *isnum) { return lua_tointegerx(L, idx, isnum); }
static inline void clua_pushglobaltable(lua_State *L) { lua_pushglobaltable(L); }
static inline void clua_remove(lua_State *L, int idx) { lua_remove(L, idx); }
static inline void clua_insert(lua_State *L, int idx) { lua_insert(L, idx); }
static inline void *clua_newuserdata(lua_State *L, size_t size) { return lua_newuserdatauv(L, size, 1); }

/// Loads `chunk` named `name` (mode "t") and leaves the function or an error message.
int clua_loadbuffer(lua_State *L, const char *chunk, size_t len, const char *name);
/// A message handler for pcall that appends a traceback.
int clua_traceback(lua_State *L);
/// The source and line of the Lua function `level` calls up, as
/// `lua_getinfo` reports them. Returns 0 when there is none.
int clua_caller(lua_State *L, int level, const char **source, int *line);
/// Opens the standard libraries.
void clua_openlibs(lua_State *L);

#endif
