/*
 * Runtime for programs compiled by scrippy.
 *
 * Values are tagged unions. Heap objects (strings, lists, maps, closures and
 * frames) are allocated from malloc and reclaimed by a stop-the-world
 * mark-sweep collector. Collection only runs at statement boundaries, and
 * every allocation is parked on a temporary root stack until the statement
 * ends, so a collection started inside a called function cannot lose a value
 * that the caller is still holding.
 */

#include <math.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef struct Obj Obj;
typedef struct Frame Frame;
typedef struct List List;
typedef struct Map Map;
typedef struct Fn Fn;
typedef struct Native Native;

typedef enum {
    T_UNDEF,
    T_NULL,
    T_BOOL,
    T_INT,
    T_FLOAT,
    T_STR,
    T_LIST,
    T_MAP,
    T_FN,
    T_NATIVE
} Type;

typedef struct {
    Type type;
    union {
        bool b;
        int64_t i;
        double f;
        Obj *o;
        const Native *native;
    } as;
} Value;

typedef struct {
    int line;
    int col;
} Loc;

typedef Value (*FnCode)(Frame *closure, Value *args, int argc);
typedef Value (*NativeCode)(Value *args, int argc, Loc loc);

struct Native {
    const char *name;
    int min_args;
    int max_args;
    NativeCode code;
};

typedef enum {
    O_STR,
    O_LIST,
    O_MAP,
    O_FN,
    O_FRAME
} ObjKind;

struct Obj {
    Obj *next;
    ObjKind kind;
    bool mark;
};

typedef struct {
    Obj h;
    size_t len;
    char *data;
} Str;

struct List {
    Obj h;
    size_t len;
    size_t cap;
    Value *items;
};

typedef struct {
    char *key;
    size_t len;
    Value value;
} MapEntry;

struct Map {
    Obj h;
    size_t len;
    size_t cap;
    MapEntry *entries;
};

struct Fn {
    Obj h;
    FnCode code;
    Frame *closure;
    const char *name;
};

struct Frame {
    Obj h;
    Frame *parent;
    size_t nslots;
    Value *slots;
    const char *name;
    Loc call;
};

enum {
    OP_ADD = 1,
    OP_SUB,
    OP_MUL,
    OP_DIV,
    OP_REM,
    OP_EQ,
    OP_NE,
    OP_LT,
    OP_LE,
    OP_GT,
    OP_GE,
    OP_RANGE,
    OP_RANGE_EXCL
};

typedef struct {
    char *data;
    size_t len;
    size_t cap;
} Buf;

static Obj *g_objects;
static size_t g_allocs;
static Frame **g_frames;
static size_t g_frame_len;
static size_t g_frame_cap;
static Obj **g_temps;
static size_t g_temp_len;
static size_t g_temp_cap;
static const char *g_file = "<script>";
static int g_call_depth;
static Loc g_call_loc;
static uint64_t g_rng = 0x9e3779b97f4a7c15ULL;

_Noreturn static void rt_fail(Loc loc, const char *fmt, ...);
static void rt_temp_push(Obj *obj);
static void rt_collect(void);

static void *xmalloc(size_t size) {
    void *p = malloc(size ? size : 1);
    if (p == NULL) {
        fputs("scrippy: out of memory\n", stderr);
        exit(1);
    }
    return p;
}

static void *xrealloc(void *ptr, size_t size) {
    void *p = realloc(ptr, size ? size : 1);
    if (p == NULL) {
        fputs("scrippy: out of memory\n", stderr);
        exit(1);
    }
    return p;
}

static void *alloc_obj(size_t size, ObjKind kind) {
    Obj *obj = xmalloc(size);
    obj->next = g_objects;
    obj->kind = kind;
    obj->mark = false;
    g_objects = obj;
    g_allocs++;
    rt_temp_push(obj);
    return obj;
}

static void mark_obj(Obj *obj);

static void mark_obj(Obj *obj) {
    static Obj **stack;
    static size_t len;
    static size_t cap;

    if (obj == NULL || obj->mark) {
        return;
    }
    obj->mark = true;
    if (len == cap) {
        cap = cap ? cap * 2 : 256;
        stack = xrealloc(stack, cap * sizeof(*stack));
    }
    stack[len++] = obj;

    while (len > 0) {
        Obj *cur = stack[--len];
        switch (cur->kind) {
        case O_STR:
            break;
        case O_LIST: {
            List *list = (List *)cur;
            for (size_t i = 0; i < list->len; i++) {
                Value item = list->items[i];
                if ((item.type == T_STR || item.type == T_LIST || item.type == T_MAP
                        || item.type == T_FN) && !item.as.o->mark) {
                    item.as.o->mark = true;
                    if (len == cap) {
                        cap = cap ? cap * 2 : 256;
                        stack = xrealloc(stack, cap * sizeof(*stack));
                    }
                    stack[len++] = item.as.o;
                }
            }
            break;
        }
        case O_MAP: {
            Map *map = (Map *)cur;
            for (size_t i = 0; i < map->len; i++) {
                Value item = map->entries[i].value;
                if ((item.type == T_STR || item.type == T_LIST || item.type == T_MAP
                        || item.type == T_FN) && !item.as.o->mark) {
                    item.as.o->mark = true;
                    if (len == cap) {
                        cap = cap ? cap * 2 : 256;
                        stack = xrealloc(stack, cap * sizeof(*stack));
                    }
                    stack[len++] = item.as.o;
                }
            }
            break;
        }
        case O_FN: {
            Fn *fn = (Fn *)cur;
            if (fn->closure != NULL && !fn->closure->h.mark) {
                fn->closure->h.mark = true;
                if (len == cap) {
                    cap = cap ? cap * 2 : 256;
                    stack = xrealloc(stack, cap * sizeof(*stack));
                }
                stack[len++] = (Obj *)fn->closure;
            }
            break;
        }
        case O_FRAME: {
            Frame *frame = (Frame *)cur;
            if (frame->parent != NULL && !frame->parent->h.mark) {
                frame->parent->h.mark = true;
                if (len == cap) {
                    cap = cap ? cap * 2 : 256;
                    stack = xrealloc(stack, cap * sizeof(*stack));
                }
                stack[len++] = (Obj *)frame->parent;
            }
            for (size_t i = 0; i < frame->nslots; i++) {
                Value slot = frame->slots[i];
                if ((slot.type == T_STR || slot.type == T_LIST || slot.type == T_MAP
                        || slot.type == T_FN) && !slot.as.o->mark) {
                    slot.as.o->mark = true;
                    if (len == cap) {
                        cap = cap ? cap * 2 : 256;
                        stack = xrealloc(stack, cap * sizeof(*stack));
                    }
                    stack[len++] = slot.as.o;
                }
            }
            break;
        }
        }
    }
}

static void free_obj(Obj *obj) {
    switch (obj->kind) {
    case O_STR: {
        Str *str = (Str *)obj;
        free(str->data);
        break;
    }
    case O_LIST: {
        List *list = (List *)obj;
        free(list->items);
        break;
    }
    case O_MAP: {
        Map *map = (Map *)obj;
        for (size_t i = 0; i < map->len; i++) {
            free(map->entries[i].key);
        }
        free(map->entries);
        break;
    }
    case O_FN:
        break;
    case O_FRAME: {
        Frame *frame = (Frame *)obj;
        free(frame->slots);
        break;
    }
    }
    free(obj);
}

static void rt_collect(void) {
    for (size_t i = 0; i < g_frame_len; i++) {
        mark_obj((Obj *)g_frames[i]);
    }
    for (size_t i = 0; i < g_temp_len; i++) {
        mark_obj(g_temps[i]);
    }

    Obj **link = &g_objects;
    while (*link != NULL) {
        Obj *obj = *link;
        if (obj->mark) {
            obj->mark = false;
            link = &obj->next;
        } else {
            *link = obj->next;
            free_obj(obj);
        }
    }
    g_allocs = 0;
}

static void rt_maybe_gc(void) {
    if (g_allocs >= 50000) {
        rt_collect();
    }
}

static void rt_temp_push(Obj *obj) {
    if (g_temp_len == g_temp_cap) {
        g_temp_cap = g_temp_cap ? g_temp_cap * 2 : 256;
        g_temps = xrealloc(g_temps, g_temp_cap * sizeof(*g_temps));
    }
    g_temps[g_temp_len++] = obj;
}

static size_t rt_mark(void) {
    return g_temp_len;
}

static void rt_reset(size_t mark) {
    if (mark <= g_temp_len) {
        g_temp_len = mark;
    }
}

static void rt_hold(Value value) {
    if (value.type == T_STR || value.type == T_LIST || value.type == T_MAP
            || value.type == T_FN) {
        rt_temp_push(value.as.o);
    }
}

_Noreturn static void rt_fail(Loc loc, const char *fmt, ...) {
    va_list args;
    fflush(stdout);
    fprintf(stderr, "%s:%d:%d: ", g_file, loc.line, loc.col);
    va_start(args, fmt);
    vfprintf(stderr, fmt, args);
    va_end(args);
    fputc('\n', stderr);

    size_t remaining = 0;
    for (size_t i = 0; i < g_frame_len; i++) {
        Frame *frame = g_frames[i];
        if (frame->name != NULL && strcmp(frame->name, "<main>") != 0) {
            remaining++;
        }
    }
    size_t shown = 0;
    for (size_t i = g_frame_len; i-- > 0;) {
        Frame *frame = g_frames[i];
        if (frame->name == NULL || strcmp(frame->name, "<main>") == 0) {
            continue;
        }
        if (shown == 20) {
            fprintf(stderr, "  ... %zu more frame(s)\n", remaining - shown);
            break;
        }
        if (frame->call.line > 0) {
            fprintf(stderr, "  at %s (%s:%d:%d)\n", frame->name, g_file,
                    frame->call.line, frame->call.col);
        } else {
            fprintf(stderr, "  at %s\n", frame->name);
        }
        shown++;
    }
    exit(1);
}

static Value make_null(void) {
    Value value;
    value.type = T_NULL;
    value.as.i = 0;
    return value;
}

static Value make_bool(bool b) {
    Value value;
    value.type = T_BOOL;
    value.as.b = b;
    return value;
}

static Value make_int(int64_t i) {
    Value value;
    value.type = T_INT;
    value.as.i = i;
    return value;
}

static Value make_float(double f) {
    Value value;
    value.type = T_FLOAT;
    value.as.f = f;
    return value;
}

static Value make_obj(Type type, Obj *obj) {
    Value value;
    value.type = type;
    value.as.o = obj;
    return value;
}

Value rt_null(void) {
    return make_null();
}

Value rt_bool(bool b) {
    return make_bool(b);
}

Value rt_int(int64_t i) {
    return make_int(i);
}

Value rt_float(double f) {
    return make_float(f);
}

Value rt_str(const char *data, size_t len) {
    Str *str = alloc_obj(sizeof(*str), O_STR);
    str->len = len;
    str->data = xmalloc(len + 1);
    if (len > 0) {
        memcpy(str->data, data, len);
    }
    str->data[len] = '\0';
    return make_obj(T_STR, (Obj *)str);
}

static const char *type_name(Value value) {
    switch (value.type) {
    case T_UNDEF:
        return "undefined";
    case T_NULL:
        return "null";
    case T_BOOL:
        return "boolean";
    case T_INT:
    case T_FLOAT:
        return "number";
    case T_STR:
        return "string";
    case T_LIST:
        return "list";
    case T_MAP:
        return "map";
    case T_FN:
    case T_NATIVE:
        return "function";
    }
    return "unknown";
}

static void buf_init(Buf *buf) {
    buf->data = NULL;
    buf->len = 0;
    buf->cap = 0;
}

static void buf_reserve(Buf *buf, size_t extra) {
    size_t need = buf->len + extra + 1;
    if (need <= buf->cap) {
        return;
    }
    size_t cap = buf->cap ? buf->cap : 64;
    while (cap < need) {
        cap *= 2;
    }
    buf->data = xrealloc(buf->data, cap);
    buf->cap = cap;
}

static void buf_put(Buf *buf, const char *data, size_t len) {
    buf_reserve(buf, len);
    if (len > 0) {
        memcpy(buf->data + buf->len, data, len);
    }
    buf->len += len;
    buf->data[buf->len] = '\0';
}

static void buf_putc(Buf *buf, char c) {
    buf_put(buf, &c, 1);
}

static void buf_puts(Buf *buf, const char *text) {
    buf_put(buf, text, strlen(text));
}

static void buf_printf(Buf *buf, const char *fmt, ...) {
    char tmp[128];
    va_list args;
    va_start(args, fmt);
    int n = vsnprintf(tmp, sizeof(tmp), fmt, args);
    va_end(args);
    if (n < 0) {
        return;
    }
    if ((size_t)n < sizeof(tmp)) {
        buf_put(buf, tmp, (size_t)n);
        return;
    }
    char *big = xmalloc((size_t)n + 1);
    va_start(args, fmt);
    vsnprintf(big, (size_t)n + 1, fmt, args);
    va_end(args);
    buf_put(buf, big, (size_t)n);
    free(big);
}

static void stringify_into(Buf *buf, Value value, int depth) {
    if (depth > 100) {
        buf_puts(buf, "...");
        return;
    }
    switch (value.type) {
    case T_UNDEF:
    case T_NULL:
        buf_puts(buf, "null");
        break;
    case T_BOOL:
        buf_puts(buf, value.as.b ? "true" : "false");
        break;
    case T_INT:
        buf_printf(buf, "%lld", (long long)value.as.i);
        break;
    case T_FLOAT: {
        double d = value.as.f;
        if (isfinite(d) && d == trunc(d) && d >= -9.0e18 && d <= 9.0e18) {
            buf_printf(buf, "%lld", (long long)d);
        } else {
            buf_printf(buf, "%.15g", d);
        }
        break;
    }
    case T_STR: {
        Str *str = (Str *)value.as.o;
        buf_put(buf, str->data, str->len);
        break;
    }
    case T_LIST: {
        List *list = (List *)value.as.o;
        buf_putc(buf, '[');
        for (size_t i = 0; i < list->len; i++) {
            if (i > 0) {
                buf_puts(buf, ", ");
            }
            stringify_into(buf, list->items[i], depth + 1);
        }
        buf_putc(buf, ']');
        break;
    }
    case T_MAP: {
        Map *map = (Map *)value.as.o;
        buf_putc(buf, '{');
        for (size_t i = 0; i < map->len; i++) {
            if (i > 0) {
                buf_puts(buf, ", ");
            }
            buf_put(buf, map->entries[i].key, map->entries[i].len);
            buf_puts(buf, ": ");
            stringify_into(buf, map->entries[i].value, depth + 1);
        }
        buf_putc(buf, '}');
        break;
    }
    case T_FN: {
        Fn *fn = (Fn *)value.as.o;
        buf_printf(buf, "<function %s>", fn->name ? fn->name : "<anonymous>");
        break;
    }
    case T_NATIVE:
        buf_printf(buf, "<function %s>", value.as.native->name);
        break;
    }
}

static Value stringify(Value value) {
    Buf buf;
    buf_init(&buf);
    stringify_into(&buf, value, 0);
    Value out = rt_str(buf.data ? buf.data : "", buf.len);
    free(buf.data);
    return out;
}

static bool truthy(Value value) {
    switch (value.type) {
    case T_UNDEF:
    case T_NULL:
        return false;
    case T_BOOL:
        return value.as.b;
    case T_INT:
        return value.as.i != 0;
    case T_FLOAT:
        return value.as.f != 0.0;
    case T_STR:
        return ((Str *)value.as.o)->len != 0;
    case T_LIST:
        return ((List *)value.as.o)->len != 0;
    case T_MAP:
        return ((Map *)value.as.o)->len != 0;
    default:
        return true;
    }
}

bool rt_truthy(Value value) {
    return truthy(value);
}

static bool parse_number(Value value, double *out) {
    if (value.type == T_INT) {
        *out = (double)value.as.i;
        return true;
    }
    if (value.type == T_FLOAT) {
        *out = value.as.f;
        return true;
    }
    if (value.type != T_STR) {
        return false;
    }
    Str *str = (Str *)value.as.o;
    char *copy = xmalloc(str->len + 1);
    memcpy(copy, str->data, str->len);
    copy[str->len] = '\0';
    char *end = NULL;
    double d = strtod(copy, &end);
    while (end != NULL && (*end == ' ' || *end == '\t' || *end == '\n'
            || *end == '\r')) {
        end++;
    }
    bool ok = end != copy && end != NULL && *end == '\0';
    free(copy);
    if (ok) {
        *out = d;
    }
    return ok;
}

static int64_t i64_from_double(double d) {
    if (isnan(d)) {
        return 0;
    }
    if (d >= 9223372036854775807.0) {
        return INT64_MAX;
    }
    if (d <= -9223372036854775808.0) {
        return INT64_MIN;
    }
    return (int64_t)d;
}

static bool parse_int(Value value, int64_t *out) {
    if (value.type == T_INT) {
        *out = value.as.i;
        return true;
    }
    if (value.type == T_FLOAT) {
        *out = i64_from_double(value.as.f);
        return true;
    }
    if (value.type != T_STR) {
        return false;
    }
    Str *str = (Str *)value.as.o;
    char *copy = xmalloc(str->len + 1);
    memcpy(copy, str->data, str->len);
    copy[str->len] = '\0';
    char *start = copy;
    while (*start == ' ' || *start == '\t' || *start == '\n' || *start == '\r') {
        start++;
    }
    char *end = NULL;
    long long n = strtoll(start, &end, 10);
    while (end != NULL && (*end == ' ' || *end == '\t' || *end == '\n'
            || *end == '\r')) {
        end++;
    }
    bool ok = end != start && end != NULL && *end == '\0';
    free(copy);
    if (ok) {
        *out = (int64_t)n;
    }
    return ok;
}

static int64_t need_int(Value value, Loc loc, const char *what) {
    int64_t out;
    if (!parse_int(value, &out)) {
        rt_fail(loc, "%s must be a number but was %s", what, type_name(value));
    }
    return out;
}

static double need_number(Value value, Loc loc, const char *what) {
    double out;
    if (!parse_number(value, &out)) {
        rt_fail(loc, "%s must be a number but was %s", what, type_name(value));
    }
    return out;
}

static void need_args(const char *name, int argc, int min, int max, Loc loc) {
    if (argc < min || (max >= 0 && argc > max)) {
        if (min == max) {
            rt_fail(loc, "%s() expects %d argument(s), got %d", name, min, argc);
        } else if (max < 0) {
            rt_fail(loc, "%s() expects at least %d argument(s), got %d", name,
                    min, argc);
        } else {
            rt_fail(loc, "%s() expects %d to %d argument(s), got %d", name, min,
                    max, argc);
        }
    }
}

static List *new_list(void) {
    List *list = alloc_obj(sizeof(*list), O_LIST);
    list->len = 0;
    list->cap = 0;
    list->items = NULL;
    return list;
}

static void list_push(List *list, Value value) {
    if (list->len == list->cap) {
        list->cap = list->cap ? list->cap * 2 : 8;
        list->items = xrealloc(list->items, list->cap * sizeof(Value));
    }
    list->items[list->len++] = value;
}

Value rt_list(void) {
    return make_obj(T_LIST, (Obj *)new_list());
}

void rt_list_push_lit(Value list_value, Value item) {
    list_push((List *)list_value.as.o, item);
}

static Map *new_map(void) {
    Map *map = alloc_obj(sizeof(*map), O_MAP);
    map->len = 0;
    map->cap = 0;
    map->entries = NULL;
    return map;
}

static int64_t map_find(Map *map, const char *key, size_t len) {
    for (size_t i = 0; i < map->len; i++) {
        if (map->entries[i].len == len
                && memcmp(map->entries[i].key, key, len) == 0) {
            return (int64_t)i;
        }
    }
    return -1;
}

static Value map_get(Map *map, const char *key, size_t len, bool *found) {
    int64_t index = map_find(map, key, len);
    if (index < 0) {
        if (found != NULL) {
            *found = false;
        }
        return make_null();
    }
    if (found != NULL) {
        *found = true;
    }
    return map->entries[index].value;
}

static Value map_put(Map *map, const char *key, size_t len, Value value) {
    int64_t index = map_find(map, key, len);
    if (index >= 0) {
        Value old = map->entries[index].value;
        map->entries[index].value = value;
        return old;
    }
    if (map->len == map->cap) {
        map->cap = map->cap ? map->cap * 2 : 8;
        map->entries = xrealloc(map->entries, map->cap * sizeof(MapEntry));
    }
    char *copy = xmalloc(len + 1);
    memcpy(copy, key, len);
    copy[len] = '\0';
    map->entries[map->len].key = copy;
    map->entries[map->len].len = len;
    map->entries[map->len].value = value;
    map->len++;
    return make_null();
}

Value rt_map(void) {
    return make_obj(T_MAP, (Obj *)new_map());
}

void rt_map_put_lit(Value map_value, const char *key, size_t len, Value value) {
    map_put((Map *)map_value.as.o, key, len, value);
}

static Value key_string(Value key) {
    if (key.type == T_STR) {
        return key;
    }
    return stringify(key);
}

static Value map_get_value(Map *map, Value key, bool *found) {
    Value text = key_string(key);
    Str *str = (Str *)text.as.o;
    return map_get(map, str->data, str->len, found);
}

static Value map_put_value(Map *map, Value key, Value value) {
    Value text = key_string(key);
    Str *str = (Str *)text.as.o;
    return map_put(map, str->data, str->len, value);
}

static List *as_list(Value value, Loc loc, const char *what) {
    if (value.type != T_LIST) {
        rt_fail(loc, "%s must be a list but was %s", what, type_name(value));
    }
    return (List *)value.as.o;
}

static Map *as_map(Value value, Loc loc, const char *what) {
    if (value.type != T_MAP) {
        rt_fail(loc, "%s must be a map but was %s", what, type_name(value));
    }
    return (Map *)value.as.o;
}

static Str *as_str(Value value, Loc loc, const char *what) {
    if (value.type != T_STR) {
        rt_fail(loc, "%s must be a string but was %s", what, type_name(value));
    }
    return (Str *)value.as.o;
}

static size_t utf8_width(unsigned char c) {
    if (c < 0x80) {
        return 1;
    }
    if ((c & 0xE0) == 0xC0) {
        return 2;
    }
    if ((c & 0xF0) == 0xE0) {
        return 3;
    }
    if ((c & 0xF8) == 0xF0) {
        return 4;
    }
    return 1;
}

static size_t utf8_count(const char *text, size_t len) {
    size_t count = 0;
    for (size_t i = 0; i < len; count++) {
        i += utf8_width((unsigned char)text[i]);
    }
    return count;
}

static size_t utf8_offset(const char *text, size_t len, size_t index) {
    size_t offset = 0;
    while (index > 0 && offset < len) {
        offset += utf8_width((unsigned char)text[offset]);
        index--;
    }
    return offset;
}

static const char *find_bytes(const char *haystack, size_t haystack_len,
        const char *needle, size_t needle_len) {
    if (needle_len == 0) {
        return haystack;
    }
    for (size_t i = 0; i + needle_len <= haystack_len; i++) {
        if (memcmp(haystack + i, needle, needle_len) == 0) {
            return haystack + i;
        }
    }
    return NULL;
}

static size_t normalize_index(int64_t index, size_t size) {
    int64_t adjusted = index < 0 ? index + (int64_t)size : index;
    if (adjusted < 0 || adjusted >= (int64_t)size) {
        return SIZE_MAX;
    }
    return (size_t)adjusted;
}

static int compare_values(Value a, Value b, Loc loc) {
    bool a_number = a.type == T_INT || a.type == T_FLOAT;
    bool b_number = b.type == T_INT || b.type == T_FLOAT;
    if (a.type == T_INT && b.type == T_INT) {
        return a.as.i < b.as.i ? -1 : a.as.i > b.as.i ? 1 : 0;
    }
    if (a_number && b_number) {
        double x = a.type == T_INT ? (double)a.as.i : a.as.f;
        double y = b.type == T_INT ? (double)b.as.i : b.as.f;
        return x < y ? -1 : x > y ? 1 : 0;
    }
    if (a.type == T_STR && b.type == T_STR) {
        Str *left = (Str *)a.as.o;
        Str *right = (Str *)b.as.o;
        size_t common = left->len < right->len ? left->len : right->len;
        int cmp = common ? memcmp(left->data, right->data, common) : 0;
        if (cmp == 0) {
            cmp = left->len < right->len ? -1
                : left->len > right->len ? 1 : 0;
        }
        return cmp;
    }
    rt_fail(loc, "cannot compare %s with %s", type_name(a), type_name(b));
}

static int sort_order(Value a, Value b) {
    if (a.type == T_INT && b.type == T_INT) {
        return a.as.i < b.as.i ? -1 : a.as.i > b.as.i ? 1 : 0;
    }
    bool a_number = a.type == T_INT || a.type == T_FLOAT;
    bool b_number = b.type == T_INT || b.type == T_FLOAT;
    if (a_number && b_number) {
        double x = a.type == T_INT ? (double)a.as.i : a.as.f;
        double y = b.type == T_INT ? (double)b.as.i : b.as.f;
        return x < y ? -1 : x > y ? 1 : 0;
    }
    if (a.type == T_STR && b.type == T_STR) {
        Str *left = (Str *)a.as.o;
        Str *right = (Str *)b.as.o;
        size_t common = left->len < right->len ? left->len : right->len;
        int cmp = common ? memcmp(left->data, right->data, common) : 0;
        if (cmp == 0) {
            cmp = left->len < right->len ? -1
                : left->len > right->len ? 1 : 0;
        }
        return cmp;
    }
    Buf left;
    Buf right;
    buf_init(&left);
    buf_init(&right);
    stringify_into(&left, a, 0);
    stringify_into(&right, b, 0);
    size_t common = left.len < right.len ? left.len : right.len;
    int cmp = common ? memcmp(left.data, right.data, common) : 0;
    if (cmp == 0) {
        cmp = left.len < right.len ? -1 : left.len > right.len ? 1 : 0;
    }
    free(left.data);
    free(right.data);
    return cmp;
}

static bool value_equal(Value a, Value b, Loc loc, int depth) {
    if (depth > 200) {
        rt_fail(loc, "comparison is nested too deeply");
    }
    if (a.type == T_NULL || a.type == T_UNDEF || b.type == T_NULL
            || b.type == T_UNDEF) {
        return (a.type == T_NULL || a.type == T_UNDEF)
            && (b.type == T_NULL || b.type == T_UNDEF);
    }
    if (a.type == T_INT && b.type == T_INT) {
        return a.as.i == b.as.i;
    }
    if ((a.type == T_INT || a.type == T_FLOAT)
            && (b.type == T_INT || b.type == T_FLOAT)) {
        double x = a.type == T_INT ? (double)a.as.i : a.as.f;
        double y = b.type == T_INT ? (double)b.as.i : b.as.f;
        return x == y;
    }
    if (a.type != b.type) {
        return false;
    }
    switch (a.type) {
    case T_BOOL:
        return a.as.b == b.as.b;
    case T_STR: {
        Str *x = (Str *)a.as.o;
        Str *y = (Str *)b.as.o;
        return x->len == y->len && memcmp(x->data, y->data, x->len) == 0;
    }
    case T_LIST: {
        List *x = (List *)a.as.o;
        List *y = (List *)b.as.o;
        if (x->len != y->len) {
            return false;
        }
        for (size_t i = 0; i < x->len; i++) {
            if (!value_equal(x->items[i], y->items[i], loc, depth + 1)) {
                return false;
            }
        }
        return true;
    }
    case T_MAP: {
        Map *x = (Map *)a.as.o;
        Map *y = (Map *)b.as.o;
        if (x->len != y->len) {
            return false;
        }
        for (size_t i = 0; i < x->len; i++) {
            bool found = false;
            Value other = map_get(y, x->entries[i].key, x->entries[i].len,
                    &found);
            if (!found
                    || !value_equal(x->entries[i].value, other, loc,
                            depth + 1)) {
                return false;
            }
        }
        return true;
    }
    case T_FN:
    case T_NATIVE:
        return a.as.native == b.as.native;
    default:
        return false;
    }
}

static Value add_values(Value a, Value b, Loc loc) {
    if (a.type == T_STR || b.type == T_STR) {
        Buf buf;
        buf_init(&buf);
        stringify_into(&buf, a, 0);
        stringify_into(&buf, b, 0);
        Value out = rt_str(buf.data ? buf.data : "", buf.len);
        free(buf.data);
        return out;
    }
    double x;
    double y;
    if (!parse_number(a, &x) || !parse_number(b, &y)) {
        rt_fail(loc, "cannot add %s and %s", type_name(a), type_name(b));
    }
    if (a.type == T_INT && b.type == T_INT) {
        return make_int((int64_t)((uint64_t)a.as.i + (uint64_t)b.as.i));
    }
    return make_float(x + y);
}

static Value arithmetic(int op, Value a, Value b, Loc loc) {
    double x;
    double y;
    if (!parse_number(a, &x) || !parse_number(b, &y)) {
        rt_fail(loc, "operator needs numbers, got %s and %s", type_name(a),
                type_name(b));
    }
    bool ints = a.type == T_INT && b.type == T_INT;
    switch (op) {
    case OP_SUB:
        if (ints) {
            return make_int((int64_t)((uint64_t)a.as.i - (uint64_t)b.as.i));
        }
        return make_float(x - y);
    case OP_MUL:
        if (ints) {
            return make_int((int64_t)((uint64_t)a.as.i * (uint64_t)b.as.i));
        }
        return make_float(x * y);
    case OP_DIV:
        if (y == 0.0) {
            rt_fail(loc, "division by zero");
        }
        if (ints) {
            int64_t left = a.as.i;
            int64_t right = b.as.i;
            if (right == -1 && left == INT64_MIN) {
                return make_int(INT64_MIN);
            }
            if (left % right == 0) {
                return make_int(left / right);
            }
        }
        return make_float(x / y);
    case OP_REM:
        if (y == 0.0) {
            rt_fail(loc, "modulo by zero");
        }
        if (ints) {
            int64_t left = a.as.i;
            int64_t right = b.as.i;
            if (right == -1) {
                return make_int(0);
            }
            return make_int(left % right);
        }
        return make_float(fmod(x, y));
    }
    rt_fail(loc, "unknown arithmetic operator");
}

static Value make_range(Value from, Value to, bool inclusive, Loc loc) {
    double start_d = need_number(from, loc, "range start");
    double end_d = need_number(to, loc, "range end");
    int64_t start = i64_from_double(start_d);
    int64_t end = i64_from_double(end_d);
    int64_t step = start <= end ? 1 : -1;

    // long double keeps the distance exact enough to reject huge ranges
    // without overflowing int64 when the endpoints are near the limits
    long double diff = (long double)end - (long double)start;
    long double span = diff < 0 ? -diff : diff;
    long double total = inclusive ? span + 1 : span;
    if (total > 1000000.0L) {
        rt_fail(loc, "range is too large (%.0Lf elements)", total);
    }

    uint64_t count = (uint64_t)total;
    List *list = new_list();
    int64_t value = start;
    for (uint64_t i = 0; i < count; i++) {
        list_push(list, make_int(value));
        if (i + 1 < count) {
            value += step;
        }
    }
    return make_obj(T_LIST, (Obj *)list);
}

Value rt_binary(int op, Value a, Value b, Loc loc) {
    switch (op) {
    case OP_ADD:
        return add_values(a, b, loc);
    case OP_SUB:
    case OP_MUL:
    case OP_DIV:
    case OP_REM:
        return arithmetic(op, a, b, loc);
    case OP_EQ:
        return make_bool(value_equal(a, b, loc, 0));
    case OP_NE:
        return make_bool(!value_equal(a, b, loc, 0));
    case OP_LT:
        return make_bool(compare_values(a, b, loc) < 0);
    case OP_LE:
        return make_bool(compare_values(a, b, loc) <= 0);
    case OP_GT:
        return make_bool(compare_values(a, b, loc) > 0);
    case OP_GE:
        return make_bool(compare_values(a, b, loc) >= 0);
    case OP_RANGE:
        return make_range(a, b, true, loc);
    case OP_RANGE_EXCL:
        return make_range(a, b, false, loc);
    }
    rt_fail(loc, "unknown operator");
}

Value rt_neg(Value value, Loc loc) {
    double number = need_number(value, loc, "operand");
    if (value.type == T_INT) {
        return make_int((int64_t)(0ULL - (uint64_t)value.as.i));
    }
    return make_float(-number);
}

Value rt_not(Value value) {
    return make_bool(!truthy(value));
}

Value rt_index_get(Value object, Value index, Loc loc) {
    if (object.type == T_LIST) {
        List *list = (List *)object.as.o;
        int64_t raw = need_int(index, loc, "index");
        size_t at = normalize_index(raw, list->len);
        if (at == SIZE_MAX) {
            rt_fail(loc, "index %lld is out of bounds (size %zu)",
                    (long long)raw, list->len);
        }
        return list->items[at];
    }
    if (object.type == T_STR) {
        Str *str = (Str *)object.as.o;
        int64_t raw = need_int(index, loc, "index");
        size_t count = utf8_count(str->data, str->len);
        size_t at = normalize_index(raw, count);
        if (at == SIZE_MAX) {
            rt_fail(loc, "index %lld is out of bounds (size %zu)",
                    (long long)raw, count);
        }
        size_t start = utf8_offset(str->data, str->len, at);
        size_t width = utf8_width((unsigned char)str->data[start]);
        return rt_str(str->data + start, width);
    }
    if (object.type == T_MAP) {
        return map_get_value((Map *)object.as.o, index, NULL);
    }
    rt_fail(loc, "cannot index into %s", type_name(object));
}

void rt_index_set(Value object, Value index, Value value, Loc loc) {
    if (object.type == T_LIST) {
        List *list = (List *)object.as.o;
        int64_t raw = need_int(index, loc, "index");
        size_t at = normalize_index(raw, list->len);
        if (at == SIZE_MAX) {
            rt_fail(loc, "index %lld is out of bounds (size %zu)",
                    (long long)raw, list->len);
        }
        list->items[at] = value;
        return;
    }
    if (object.type == T_MAP) {
        map_put_value((Map *)object.as.o, index, value);
        return;
    }
    rt_fail(loc, "cannot assign into %s", type_name(object));
}

static Value list_member(List *list, const char *name, Loc loc) {
    if (strcmp(name, "size") == 0 || strcmp(name, "length") == 0) {
        return make_int((int64_t)list->len);
    }
    if (strcmp(name, "isEmpty") == 0) {
        return make_bool(list->len == 0);
    }
    if (strcmp(name, "first") == 0) {
        return list->len ? list->items[0] : make_null();
    }
    if (strcmp(name, "last") == 0) {
        return list->len ? list->items[list->len - 1] : make_null();
    }
    rt_fail(loc, "unknown member '%s' on list", name);
}

static Value string_member(Str *str, const char *name, Loc loc) {
    if (strcmp(name, "size") == 0 || strcmp(name, "length") == 0) {
        return make_int((int64_t)utf8_count(str->data, str->len));
    }
    if (strcmp(name, "isEmpty") == 0) {
        return make_bool(str->len == 0);
    }
    rt_fail(loc, "unknown member '%s' on string", name);
}

Value rt_member_get(Value object, const char *name, Loc loc) {
    if (object.type == T_MAP) {
        return map_get((Map *)object.as.o, name, strlen(name), NULL);
    }
    if (object.type == T_LIST) {
        return list_member((List *)object.as.o, name, loc);
    }
    if (object.type == T_STR) {
        return string_member((Str *)object.as.o, name, loc);
    }
    rt_fail(loc, "unknown property '%s' on %s", name, type_name(object));
}

void rt_member_set(Value object, const char *name, Value value, Loc loc) {
    if (object.type == T_MAP) {
        map_put((Map *)object.as.o, name, strlen(name), value);
        return;
    }
    rt_fail(loc, "cannot set property '%s' on %s", name, type_name(object));
}

static Value copy_list(List *source) {
    List *copy = new_list();
    for (size_t i = 0; i < source->len; i++) {
        list_push(copy, source->items[i]);
    }
    return make_obj(T_LIST, (Obj *)copy);
}

static Value string_slice(Str *str, size_t from, size_t to) {
    size_t count = utf8_count(str->data, str->len);
    if (from > count) {
        from = count;
    }
    if (to > count) {
        to = count;
    }
    if (to < from) {
        to = from;
    }
    size_t start = utf8_offset(str->data, str->len, from);
    size_t end = utf8_offset(str->data, str->len, to);
    return rt_str(str->data + start, end - start);
}

static Value string_split(Str *str, Str *separator) {
    List *out = new_list();
    if (separator->len == 0) {
        size_t offset = 0;
        while (offset < str->len) {
            size_t width = utf8_width((unsigned char)str->data[offset]);
            list_push(out, rt_str(str->data + offset, width));
            offset += width;
        }
        return make_obj(T_LIST, (Obj *)out);
    }
    size_t start = 0;
    for (size_t i = 0; i + separator->len <= str->len;) {
        if (memcmp(str->data + i, separator->data, separator->len) == 0) {
            list_push(out, rt_str(str->data + start, i - start));
            i += separator->len;
            start = i;
        } else {
            i++;
        }
    }
    list_push(out, rt_str(str->data + start, str->len - start));
    return make_obj(T_LIST, (Obj *)out);
}

static Value string_replace(Str *str, Str *from, Str *to) {
    if (from->len == 0) {
        return rt_str(str->data, str->len);
    }
    Buf buf;
    buf_init(&buf);
    size_t i = 0;
    while (i < str->len) {
        if (i + from->len <= str->len
                && memcmp(str->data + i, from->data, from->len) == 0) {
            buf_put(&buf, to->data, to->len);
            i += from->len;
        } else {
            buf_putc(&buf, str->data[i]);
            i++;
        }
    }
    Value out = rt_str(buf.data ? buf.data : "", buf.len);
    free(buf.data);
    return out;
}

static Value string_upper(Str *str) {
    char *copy = xmalloc(str->len);
    for (size_t i = 0; i < str->len; i++) {
        char c = str->data[i];
        copy[i] = c >= 'a' && c <= 'z' ? (char)(c - 'a' + 'A') : c;
    }
    Value out = rt_str(copy, str->len);
    free(copy);
    return out;
}

static Value string_lower(Str *str) {
    char *copy = xmalloc(str->len);
    for (size_t i = 0; i < str->len; i++) {
        char c = str->data[i];
        copy[i] = c >= 'A' && c <= 'Z' ? (char)(c - 'A' + 'a') : c;
    }
    Value out = rt_str(copy, str->len);
    free(copy);
    return out;
}

static Value string_trim(Str *str) {
    size_t start = 0;
    size_t end = str->len;
    while (start < end && (str->data[start] == ' ' || str->data[start] == '\t'
            || str->data[start] == '\n' || str->data[start] == '\r')) {
        start++;
    }
    while (end > start && (str->data[end - 1] == ' ' || str->data[end - 1] == '\t'
            || str->data[end - 1] == '\n' || str->data[end - 1] == '\r')) {
        end--;
    }
    return rt_str(str->data + start, end - start);
}

static Value string_index_of(Str *str, Str *needle) {
    if (needle->len == 0) {
        return make_int(0);
    }
    for (size_t i = 0; i + needle->len <= str->len; i++) {
        if (memcmp(str->data + i, needle->data, needle->len) == 0) {
            return make_int((int64_t)utf8_count(str->data, i));
        }
    }
    return make_int(-1);
}

static Value list_join(List *list, Str *separator) {
    Buf buf;
    buf_init(&buf);
    for (size_t i = 0; i < list->len; i++) {
        if (i > 0) {
            buf_put(&buf, separator->data, separator->len);
        }
        stringify_into(&buf, list->items[i], 0);
    }
    Value out = rt_str(buf.data ? buf.data : "", buf.len);
    free(buf.data);
    return out;
}

static int sort_pair(const void *a, const void *b) {
    return sort_order(*(const Value *)a, *(const Value *)b);
}

static void sort_list(List *list) {
    if (list->len > 1) {
        qsort(list->items, list->len, sizeof(Value), sort_pair);
    }
}

static Value call_map_method(Map *map, const char *name, Value *args, int argc,
        Loc loc) {
    if (strcmp(name, "get") == 0) {
        need_args(name, argc, 1, 2, loc);
        bool found = false;
        Value value = map_get_value(map, args[0], &found);
        return found ? value : (argc > 1 ? args[1] : make_null());
    }
    if (strcmp(name, "put") == 0 || strcmp(name, "set") == 0) {
        need_args(name, argc, 2, 2, loc);
        return map_put_value(map, args[0], args[1]);
    }
    if (strcmp(name, "containsKey") == 0 || strcmp(name, "contains") == 0) {
        need_args(name, argc, 1, 1, loc);
        bool found = false;
        map_get_value(map, args[0], &found);
        return make_bool(found);
    }
    if (strcmp(name, "remove") == 0) {
        need_args(name, argc, 1, 1, loc);
        Value key = key_string(args[0]);
        Str *text = (Str *)key.as.o;
        int64_t at = map_find(map, text->data, text->len);
        if (at < 0) {
            return make_null();
        }
        Value old = map->entries[at].value;
        free(map->entries[at].key);
        for (size_t i = (size_t)at + 1; i < map->len; i++) {
            map->entries[i - 1] = map->entries[i];
        }
        map->len--;
        return old;
    }
    if (strcmp(name, "getOrDefault") == 0) {
        need_args(name, argc, 1, 2, loc);
        bool found = false;
        Value value = map_get_value(map, args[0], &found);
        return found ? value : (argc > 1 ? args[1] : make_null());
    }
    if (strcmp(name, "keys") == 0 || strcmp(name, "values") == 0) {
        need_args(name, argc, 0, 0, loc);
        List *out = new_list();
        for (size_t i = 0; i < map->len; i++) {
            if (strcmp(name, "keys") == 0) {
                list_push(out, rt_str(map->entries[i].key,
                        map->entries[i].len));
            } else {
                list_push(out, map->entries[i].value);
            }
        }
        return make_obj(T_LIST, (Obj *)out);
    }
    if (strcmp(name, "size") == 0) {
        need_args(name, argc, 0, 0, loc);
        return make_int((int64_t)map->len);
    }
    if (strcmp(name, "isEmpty") == 0) {
        need_args(name, argc, 0, 0, loc);
        return make_bool(map->len == 0);
    }
    if (argc == 0) {
        bool found = false;
        Value value = map_get(map, name, strlen(name), &found);
        if (found) {
            return value;
        }
    }
    rt_fail(loc, "unknown method '%s' on map", name);
}

static Value call_list_method(List *list, const char *name, Value *args,
        int argc, Loc loc) {
    if (strcmp(name, "add") == 0) {
        need_args(name, argc, 1, 1, loc);
        list_push(list, args[0]);
        return make_null();
    }
    if (strcmp(name, "get") == 0) {
        need_args(name, argc, 1, 1, loc);
        int64_t raw = need_int(args[0], loc, "index");
        size_t at = normalize_index(raw, list->len);
        if (at == SIZE_MAX) {
            rt_fail(loc, "index %lld is out of bounds (size %zu)",
                    (long long)raw, list->len);
        }
        return list->items[at];
    }
    if (strcmp(name, "set") == 0) {
        need_args(name, argc, 2, 2, loc);
        int64_t raw = need_int(args[0], loc, "index");
        size_t at = normalize_index(raw, list->len);
        if (at == SIZE_MAX) {
            rt_fail(loc, "index %lld is out of bounds (size %zu)",
                    (long long)raw, list->len);
        }
        list->items[at] = args[1];
        return make_null();
    }
    if (strcmp(name, "remove") == 0) {
        need_args(name, argc, 1, 1, loc);
        if (args[0].type == T_INT || args[0].type == T_FLOAT) {
            int64_t raw = need_int(args[0], loc, "index");
            size_t at = normalize_index(raw, list->len);
            if (at == SIZE_MAX) {
                rt_fail(loc, "index %lld is out of bounds (size %zu)",
                        (long long)raw, list->len);
            }
            for (size_t i = at + 1; i < list->len; i++) {
                list->items[i - 1] = list->items[i];
            }
            list->len--;
            return make_null();
        }
        size_t write = 0;
        for (size_t i = 0; i < list->len; i++) {
            if (!value_equal(list->items[i], args[0], loc, 0)) {
                list->items[write++] = list->items[i];
            }
        }
        list->len = write;
        return make_null();
    }
    if (strcmp(name, "contains") == 0) {
        need_args(name, argc, 1, 1, loc);
        for (size_t i = 0; i < list->len; i++) {
            if (value_equal(list->items[i], args[0], loc, 0)) {
                return make_bool(true);
            }
        }
        return make_bool(false);
    }
    if (strcmp(name, "indexOf") == 0) {
        need_args(name, argc, 1, 1, loc);
        for (size_t i = 0; i < list->len; i++) {
            if (value_equal(list->items[i], args[0], loc, 0)) {
                return make_int((int64_t)i);
            }
        }
        return make_int(-1);
    }
    if (strcmp(name, "slice") == 0) {
        need_args(name, argc, 2, 2, loc);
        int64_t from = need_int(args[0], loc, "start");
        int64_t to = need_int(args[1], loc, "end");
        int64_t size = (int64_t)list->len;
        from = from < 0 ? from + size : from;
        to = to < 0 ? to + size : to;
        if (from < 0) {
            from = 0;
        }
        if (to < 0) {
            to = 0;
        }
        if (from > size) {
            from = size;
        }
        if (to > size) {
            to = size;
        }
        if (from > to) {
            int64_t tmp = from;
            from = to;
            to = tmp;
        }
        List *out = new_list();
        for (int64_t i = from; i < to; i++) {
            list_push(out, list->items[i]);
        }
        return make_obj(T_LIST, (Obj *)out);
    }
    if (strcmp(name, "join") == 0) {
        need_args(name, argc, 1, 1, loc);
        return list_join(list, as_str(args[0], loc, "separator"));
    }
    if (strcmp(name, "sort") == 0) {
        need_args(name, argc, 0, 0, loc);
        sort_list(list);
        return make_null();
    }
    if (strcmp(name, "reverse") == 0) {
        need_args(name, argc, 0, 0, loc);
        for (size_t i = 0; i < list->len / 2; i++) {
            Value tmp = list->items[i];
            list->items[i] = list->items[list->len - 1 - i];
            list->items[list->len - 1 - i] = tmp;
        }
        return make_null();
    }
    if (strcmp(name, "size") == 0) {
        need_args(name, argc, 0, 0, loc);
        return make_int((int64_t)list->len);
    }
    if (strcmp(name, "isEmpty") == 0) {
        need_args(name, argc, 0, 0, loc);
        return make_bool(list->len == 0);
    }
    if (strcmp(name, "first") == 0) {
        need_args(name, argc, 0, 0, loc);
        return list->len ? list->items[0] : make_null();
    }
    if (strcmp(name, "last") == 0) {
        need_args(name, argc, 0, 0, loc);
        return list->len ? list->items[list->len - 1] : make_null();
    }
    rt_fail(loc, "unknown method '%s' on list", name);
}

static Value call_string_method(Str *str, const char *name, Value *args,
        int argc, Loc loc) {
    if (strcmp(name, "upper") == 0 || strcmp(name, "toUpperCase") == 0) {
        need_args(name, argc, 0, 0, loc);
        return string_upper(str);
    }
    if (strcmp(name, "lower") == 0 || strcmp(name, "toLowerCase") == 0) {
        need_args(name, argc, 0, 0, loc);
        return string_lower(str);
    }
    if (strcmp(name, "trim") == 0) {
        need_args(name, argc, 0, 0, loc);
        return string_trim(str);
    }
    if (strcmp(name, "size") == 0 || strcmp(name, "length") == 0) {
        need_args(name, argc, 0, 0, loc);
        return make_int((int64_t)utf8_count(str->data, str->len));
    }
    if (strcmp(name, "isEmpty") == 0) {
        need_args(name, argc, 0, 0, loc);
        return make_bool(str->len == 0);
    }
    if (strcmp(name, "toString") == 0) {
        need_args(name, argc, 0, 0, loc);
        return make_obj(T_STR, (Obj *)str);
    }
    if (strcmp(name, "contains") == 0 || strcmp(name, "startsWith") == 0
            || strcmp(name, "endsWith") == 0) {
        need_args(name, argc, 1, 1, loc);
        Str *needle = as_str(key_string(args[0]), loc, "argument");
        bool result;
        if (strcmp(name, "contains") == 0) {
            result = needle->len == 0
                || (needle->len <= str->len
                    && find_bytes(str->data, str->len, needle->data,
                        needle->len) != NULL);
        } else if (strcmp(name, "startsWith") == 0) {
            result = needle->len <= str->len
                && memcmp(str->data, needle->data, needle->len) == 0;
        } else {
            result = needle->len <= str->len
                && memcmp(str->data + str->len - needle->len, needle->data,
                    needle->len) == 0;
        }
        return make_bool(result);
    }
    if (strcmp(name, "indexOf") == 0) {
        need_args(name, argc, 1, 1, loc);
        Str *needle = as_str(key_string(args[0]), loc, "argument");
        return string_index_of(str, needle);
    }
    if (strcmp(name, "charAt") == 0 || strcmp(name, "at") == 0) {
        need_args(name, argc, 1, 1, loc);
        int64_t raw = need_int(args[0], loc, "index");
        size_t count = utf8_count(str->data, str->len);
        size_t at = normalize_index(raw, count);
        if (at == SIZE_MAX) {
            rt_fail(loc, "index %lld is out of bounds (size %zu)",
                    (long long)raw, count);
        }
        size_t start = utf8_offset(str->data, str->len, at);
        return rt_str(str->data + start,
                utf8_width((unsigned char)str->data[start]));
    }
    if (strcmp(name, "split") == 0) {
        need_args(name, argc, 1, 1, loc);
        Str *separator = as_str(key_string(args[0]), loc, "separator");
        return string_split(str, separator);
    }
    if (strcmp(name, "replace") == 0) {
        need_args(name, argc, 2, 2, loc);
        Str *from = as_str(key_string(args[0]), loc, "old text");
        Str *to = as_str(key_string(args[1]), loc, "new text");
        return string_replace(str, from, to);
    }
    if (strcmp(name, "substring") == 0) {
        need_args(name, argc, 1, 2, loc);
        int64_t from = need_int(args[0], loc, "start");
        size_t count = utf8_count(str->data, str->len);
        int64_t size = (int64_t)count;
        if (from < 0) {
            from += size;
        }
        if (from < 0) {
            from = 0;
        }
        if (from > size) {
            from = size;
        }
        int64_t to = size;
        if (argc > 1) {
            to = need_int(args[1], loc, "end");
            if (to < from) {
                to = from;
            }
            if (to > size) {
                to = size;
            }
        }
        return string_slice(str, (size_t)from, (size_t)to);
    }
    rt_fail(loc, "unknown method '%s' on string", name);
}

static Value call_number_method(Value number, const char *name, int argc,
        Loc loc) {
    need_args(name, argc, 0, 0, loc);
    double d = number.type == T_INT ? (double)number.as.i : number.as.f;
    if (strcmp(name, "abs") == 0) {
        if (number.type == T_INT) {
            if (number.as.i == INT64_MIN) {
                return make_int(INT64_MIN);
            }
            return make_int(number.as.i < 0 ? -number.as.i : number.as.i);
        }
        return make_float(fabs(d));
    }
    if (strcmp(name, "floor") == 0) {
        return make_int(i64_from_double(floor(d)));
    }
    if (strcmp(name, "ceil") == 0) {
        return make_int(i64_from_double(ceil(d)));
    }
    if (strcmp(name, "round") == 0) {
        return make_int(i64_from_double(floor(d + 0.5)));
    }
    if (strcmp(name, "toInt") == 0) {
        return make_int(i64_from_double(d));
    }
    if (strcmp(name, "toString") == 0) {
        return stringify(number);
    }
    rt_fail(loc, "unknown method '%s' on number", name);
}

Value rt_method_call(Value object, const char *name, Value *args, int argc,
        Loc loc) {
    if (object.type == T_NULL || object.type == T_UNDEF) {
        rt_fail(loc, "cannot call '%s' on null", name);
    }
    switch (object.type) {
    case T_MAP:
        return call_map_method((Map *)object.as.o, name, args, argc, loc);
    case T_LIST:
        return call_list_method((List *)object.as.o, name, args, argc, loc);
    case T_STR:
        return call_string_method((Str *)object.as.o, name, args, argc, loc);
    case T_INT:
    case T_FLOAT:
        return call_number_method(object, name, argc, loc);
    case T_BOOL:
        if (strcmp(name, "toString") == 0) {
            need_args(name, argc, 0, 0, loc);
            return stringify(make_bool(object.as.b));
        }
        break;
    default:
        break;
    }
    rt_fail(loc, "unknown method '%s' on %s", name, type_name(object));
}

static Value make_builtin(const Native *native) {
    Value value;
    value.type = T_NATIVE;
    value.as.native = native;
    return value;
}

static Frame *make_frame(Frame *parent, size_t nslots, const char *name,
        Loc call) {
    Frame *frame = alloc_obj(sizeof(*frame), O_FRAME);
    frame->parent = parent;
    frame->nslots = nslots;
    frame->slots = xmalloc(nslots * sizeof(Value));
    for (size_t i = 0; i < nslots; i++) {
        frame->slots[i] = make_null();
        frame->slots[i].type = T_UNDEF;
    }
    frame->name = name;
    frame->call = call;
    return frame;
}

Frame *rt_frame(Frame *parent, size_t nslots, const char *name) {
    return make_frame(parent, nslots, name, g_call_loc);
}

Loc rt_frame_loc(Frame *frame) {
    return frame->call;
}

void rt_push_frame(Frame *frame) {
    if (g_frame_len == g_frame_cap) {
        g_frame_cap = g_frame_cap ? g_frame_cap * 2 : 32;
        g_frames = xrealloc(g_frames, g_frame_cap * sizeof(*g_frames));
    }
    g_frames[g_frame_len++] = frame;
}

void rt_pop_frame(void) {
    if (g_frame_len > 0) {
        g_frame_len--;
    }
}

Frame *rt_ancestor(Frame *frame, int depth) {
    while (depth > 0 && frame != NULL) {
        frame = frame->parent;
        depth--;
    }
    if (frame == NULL) {
        rt_fail((Loc){0, 0}, "internal error: missing enclosing frame");
    }
    return frame;
}

Value rt_slot_get(Frame *frame, size_t slot, const char *name, Loc loc) {
    if (slot >= frame->nslots) {
        rt_fail(loc, "internal error: slot %zu out of range", slot);
    }
    Value value = frame->slots[slot];
    if (value.type == T_UNDEF) {
        rt_fail(loc, "variable '%s' is used before it is assigned", name);
    }
    return value;
}

void rt_slot_set(Frame *frame, size_t slot, Value value) {
    if (slot < frame->nslots) {
        frame->slots[slot] = value;
    }
}

Value rt_args_list(Value *args, int argc) {
    List *list = new_list();
    for (int i = 0; i < argc; i++) {
        list_push(list, args[i]);
    }
    return make_obj(T_LIST, (Obj *)list);
}

Value rt_closure(FnCode code, const char *name, Frame *closure) {
    Fn *fn = alloc_obj(sizeof(*fn), O_FN);
    fn->code = code;
    fn->closure = closure;
    fn->name = name;
    return make_obj(T_FN, (Obj *)fn);
}

_Noreturn void rt_arity(const char *name, int expected, int got, Loc loc) {
    rt_fail(loc, "function '%s' expects %d argument(s), got %d", name, expected,
            got);
}

_Noreturn void rt_missing(const char *name, const char *param, Loc loc) {
    rt_fail(loc, "missing argument '%s' for function '%s'", param, name);
}

Value rt_call(Value callee, Value *args, int argc, Loc loc) {
    if (callee.type == T_FN) {
        Fn *fn = (Fn *)callee.as.o;
        if (g_call_depth >= 1000) {
            rt_fail(loc, "maximum call depth (1000) exceeded");
        }
        g_call_depth++;
        Loc saved = g_call_loc;
        g_call_loc = loc;
        Value result = fn->code(fn->closure, args, argc);
        g_call_loc = saved;
        g_call_depth--;
        rt_hold(result);
        return result;
    }
    if (callee.type == T_NATIVE) {
        const Native *native = callee.as.native;
        if (argc < native->min_args) {
            rt_arity(native->name, native->min_args, argc, loc);
        }
        if (native->max_args >= 0 && argc > native->max_args) {
            rt_arity(native->name, native->max_args, argc, loc);
        }
        Value result = native->code(args, argc, loc);
        rt_hold(result);
        return result;
    }
    rt_fail(loc, "value of type %s is not callable", type_name(callee));
}

Value rt_builtin(const char *name);

int64_t rt_iter_len(Value value, Loc loc) {
    if (value.type == T_NULL || value.type == T_UNDEF) {
        return 0;
    }
    if (value.type == T_LIST) {
        return (int64_t)((List *)value.as.o)->len;
    }
    if (value.type == T_MAP) {
        return (int64_t)((Map *)value.as.o)->len;
    }
    if (value.type == T_STR) {
        Str *str = (Str *)value.as.o;
        return (int64_t)utf8_count(str->data, str->len);
    }
    rt_fail(loc, "cannot iterate over %s", type_name(value));
}

Value rt_iter_at(Value value, int64_t index, Loc loc) {
    if (value.type == T_LIST) {
        List *list = (List *)value.as.o;
        if (index < 0 || (size_t)index >= list->len) {
            rt_fail(loc, "list changed while it was being iterated");
        }
        return list->items[(size_t)index];
    }
    if (value.type == T_MAP) {
        Map *map = (Map *)value.as.o;
        if (index < 0 || (size_t)index >= map->len) {
            rt_fail(loc, "map changed while it was being iterated");
        }
        return rt_str(map->entries[(size_t)index].key,
                map->entries[(size_t)index].len);
    }
    if (value.type == T_STR) {
        Str *str = (Str *)value.as.o;
        size_t start = utf8_offset(str->data, str->len, (size_t)index);
        return rt_str(str->data + start,
                utf8_width((unsigned char)str->data[start]));
    }
    rt_fail(loc, "cannot iterate over %s", type_name(value));
}

static Value bi_str(Value *args, int argc, Loc loc) {
    (void)loc;
    need_args("str", argc, 1, 1, loc);
    return stringify(args[0]);
}

static Value bi_num(Value *args, int argc, Loc loc) {
    need_args("num", argc, 1, 1, loc);
    double d = need_number(args[0], loc, "num() argument");
    if (isfinite(d) && d == trunc(d) && d >= -9.0e18 && d <= 9.0e18) {
        return make_int(i64_from_double(d));
    }
    return make_float(d);
}

static Value bi_int(Value *args, int argc, Loc loc) {
    need_args("int", argc, 1, 1, loc);
    return make_int(need_int(args[0], loc, "int() argument"));
}

static Value bi_bool(Value *args, int argc, Loc loc) {
    need_args("bool", argc, 1, 1, loc);
    if (args[0].type == T_STR) {
        Str *str = (Str *)args[0].as.o;
        char *copy = xmalloc(str->len + 1);
        size_t start = 0;
        size_t end = str->len;
        while (start < end && (str->data[start] == ' ' || str->data[start] == '\t'
                || str->data[start] == '\n' || str->data[start] == '\r')) {
            start++;
        }
        while (end > start && (str->data[end - 1] == ' ' || str->data[end - 1] == '\t'
                || str->data[end - 1] == '\n' || str->data[end - 1] == '\r')) {
            end--;
        }
        size_t len = end - start;
        for (size_t i = 0; i < len; i++) {
            char c = str->data[start + i];
            copy[i] = c >= 'A' && c <= 'Z' ? (char)(c - 'A' + 'a') : c;
        }
        copy[len] = '\0';
        bool result = strcmp(copy, "true") == 0;
        free(copy);
        return make_bool(result);
    }
    return make_bool(truthy(args[0]));
}

static Value bi_typeof(Value *args, int argc, Loc loc) {
    need_args("typeof", argc, 1, 1, loc);
    return rt_str(type_name(args[0]), strlen(type_name(args[0])));
}

static Value bi_is_null(Value *args, int argc, Loc loc) {
    need_args("isNull", argc, 1, 1, loc);
    return make_bool(args[0].type == T_NULL || args[0].type == T_UNDEF);
}

static Value size_of(Value value, Loc loc) {
    if (value.type == T_LIST) {
        return make_int((int64_t)((List *)value.as.o)->len);
    }
    if (value.type == T_MAP) {
        return make_int((int64_t)((Map *)value.as.o)->len);
    }
    if (value.type == T_STR) {
        Str *str = (Str *)value.as.o;
        return make_int((int64_t)utf8_count(str->data, str->len));
    }
    rt_fail(loc, "len() expects a list, map or string, got %s",
            type_name(value));
}

static Value bi_len(Value *args, int argc, Loc loc) {
    need_args("len", argc, 1, 1, loc);
    return size_of(args[0], loc);
}

static Value bi_list(Value *args, int argc, Loc loc) {
    (void)loc;
    List *list = new_list();
    for (int i = 0; i < argc; i++) {
        list_push(list, args[i]);
    }
    return make_obj(T_LIST, (Obj *)list);
}

static Value bi_keys(Value *args, int argc, Loc loc) {
    need_args("keys", argc, 1, 1, loc);
    Map *map = as_map(args[0], loc, "keys() argument");
    List *out = new_list();
    for (size_t i = 0; i < map->len; i++) {
        list_push(out, rt_str(map->entries[i].key, map->entries[i].len));
    }
    return make_obj(T_LIST, (Obj *)out);
}

static Value bi_values(Value *args, int argc, Loc loc) {
    need_args("values", argc, 1, 1, loc);
    Map *map = as_map(args[0], loc, "values() argument");
    List *out = new_list();
    for (size_t i = 0; i < map->len; i++) {
        list_push(out, map->entries[i].value);
    }
    return make_obj(T_LIST, (Obj *)out);
}

static Value bi_contains(Value *args, int argc, Loc loc) {
    need_args("contains", argc, 2, 2, loc);
    if (args[0].type == T_STR) {
        Str *haystack = (Str *)args[0].as.o;
        Str *needle = as_str(key_string(args[1]), loc, "needle");
        if (needle->len == 0) {
            return make_bool(true);
        }
        return make_bool(find_bytes(haystack->data, haystack->len, needle->data,
                    needle->len) != NULL);
    }
    if (args[0].type == T_LIST) {
        List *list = (List *)args[0].as.o;
        for (size_t i = 0; i < list->len; i++) {
            if (value_equal(list->items[i], args[1], loc, 0)) {
                return make_bool(true);
            }
        }
        return make_bool(false);
    }
    if (args[0].type == T_MAP) {
        bool found = false;
        map_get_value((Map *)args[0].as.o, args[1], &found);
        return make_bool(found);
    }
    return make_bool(false);
}

static Value bi_add(Value *args, int argc, Loc loc) {
    need_args("add", argc, 2, 2, loc);
    List *list = as_list(args[0], loc, "add() argument");
    list_push(list, args[1]);
    return args[0];
}

static Value bi_remove(Value *args, int argc, Loc loc) {
    need_args("remove", argc, 2, 2, loc);
    List *list = as_list(args[0], loc, "remove() argument");
    size_t write = 0;
    for (size_t i = 0; i < list->len; i++) {
        if (!value_equal(list->items[i], args[1], loc, 0)) {
            list->items[write++] = list->items[i];
        }
    }
    list->len = write;
    return args[0];
}

static Value bi_sort(Value *args, int argc, Loc loc) {
    need_args("sort", argc, 1, 1, loc);
    List *list = as_list(args[0], loc, "sort() argument");
    Value copy = copy_list(list);
    sort_list((List *)copy.as.o);
    return copy;
}

static Value bi_reverse(Value *args, int argc, Loc loc) {
    need_args("reverse", argc, 1, 1, loc);
    List *list = as_list(args[0], loc, "reverse() argument");
    Value copy = copy_list(list);
    List *out = (List *)copy.as.o;
    for (size_t i = 0; i < out->len / 2; i++) {
        Value tmp = out->items[i];
        out->items[i] = out->items[out->len - 1 - i];
        out->items[out->len - 1 - i] = tmp;
    }
    return copy;
}

static Value bi_join(Value *args, int argc, Loc loc) {
    need_args("join", argc, 2, 2, loc);
    List *list = as_list(args[0], loc, "join() argument");
    Str *separator = as_str(key_string(args[1]), loc, "separator");
    return list_join(list, separator);
}

static Value bi_split(Value *args, int argc, Loc loc) {
    need_args("split", argc, 2, 2, loc);
    Str *str = as_str(key_string(args[0]), loc, "split() argument");
    Str *separator = as_str(key_string(args[1]), loc, "separator");
    return string_split(str, separator);
}

static Value bi_range(Value *args, int argc, Loc loc) {
    need_args("range", argc, 2, 2, loc);
    return make_range(args[0], args[1], true, loc);
}

static Value bi_upper(Value *args, int argc, Loc loc) {
    need_args("upper", argc, 1, 1, loc);
    Str *str = as_str(key_string(args[0]), loc, "argument");
    return string_upper(str);
}

static Value bi_lower(Value *args, int argc, Loc loc) {
    need_args("lower", argc, 1, 1, loc);
    Str *str = as_str(key_string(args[0]), loc, "argument");
    return string_lower(str);
}

static Value bi_trim(Value *args, int argc, Loc loc) {
    need_args("trim", argc, 1, 1, loc);
    Str *str = as_str(key_string(args[0]), loc, "argument");
    return string_trim(str);
}

static Value bi_replace(Value *args, int argc, Loc loc) {
    need_args("replace", argc, 3, 3, loc);
    Str *str = as_str(key_string(args[0]), loc, "argument");
    Str *from = as_str(key_string(args[1]), loc, "old text");
    Str *to = as_str(key_string(args[2]), loc, "new text");
    return string_replace(str, from, to);
}

static Value bi_ends(Value *args, int argc, Loc loc, bool starts) {
    need_args(starts ? "startsWith" : "endsWith", argc, 2, 2, loc);
    Str *str = as_str(key_string(args[0]), loc, "argument");
    Str *needle = as_str(key_string(args[1]), loc, "argument");
    bool result = starts
        ? (needle->len <= str->len
                && memcmp(str->data, needle->data, needle->len) == 0)
        : (needle->len <= str->len
                && memcmp(str->data + str->len - needle->len, needle->data,
                    needle->len) == 0);
    return make_bool(result);
}

static Value bi_starts_with(Value *args, int argc, Loc loc) {
    return bi_ends(args, argc, loc, true);
}

static Value bi_ends_with(Value *args, int argc, Loc loc) {
    return bi_ends(args, argc, loc, false);
}

static Value bi_substring(Value *args, int argc, Loc loc) {
    need_args("substring", argc, 2, 3, loc);
    Str *str = as_str(key_string(args[0]), loc, "argument");
    size_t count = utf8_count(str->data, str->len);
    int64_t from = need_int(args[1], loc, "start");
    int64_t size = (int64_t)count;
    if (from < 0) {
        from += size;
    }
    if (from < 0) {
        from = 0;
    }
    if (from > size) {
        from = size;
    }
    int64_t to = size;
    if (argc > 2) {
        to = need_int(args[2], loc, "end");
        if (to < from) {
            to = from;
        }
        if (to > size) {
            to = size;
        }
    }
    return string_slice(str, (size_t)from, (size_t)to);
}

static Value bi_color(Value *args, int argc, Loc loc) {
    need_args("color", argc, 1, 1, loc);
    Str *str = as_str(key_string(args[0]), loc, "argument");
    Buf buf;
    buf_init(&buf);
    for (size_t i = 0; i < str->len; i++) {
        if (str->data[i] == '&') {
            buf_put(&buf, "\xC2\xA7", 2);
        } else {
            buf_putc(&buf, str->data[i]);
        }
    }
    Value out = rt_str(buf.data ? buf.data : "", buf.len);
    free(buf.data);
    return out;
}

static Value bi_abs(Value *args, int argc, Loc loc) {
    need_args("abs", argc, 1, 1, loc);
    if (args[0].type == T_INT) {
        int64_t value = args[0].as.i;
        if (value == INT64_MIN) {
            return make_int(INT64_MIN);
        }
        return make_int(value < 0 ? -value : value);
    }
    return make_float(fabs(need_number(args[0], loc, "abs() argument")));
}

static Value bi_floor(Value *args, int argc, Loc loc) {
    need_args("floor", argc, 1, 1, loc);
    return make_int(i64_from_double(
                floor(need_number(args[0], loc, "floor()"))));
}

static Value bi_ceil(Value *args, int argc, Loc loc) {
    need_args("ceil", argc, 1, 1, loc);
    return make_int(i64_from_double(
                ceil(need_number(args[0], loc, "ceil()"))));
}

static Value bi_round(Value *args, int argc, Loc loc) {
    need_args("round", argc, 1, 1, loc);
    return make_int(i64_from_double(
                floor(need_number(args[0], loc, "round()") + 0.5)));
}

static Value bi_sqrt(Value *args, int argc, Loc loc) {
    need_args("sqrt", argc, 1, 1, loc);
    return make_float(sqrt(need_number(args[0], loc, "sqrt()")));
}

static Value bi_pow(Value *args, int argc, Loc loc) {
    need_args("pow", argc, 2, 2, loc);
    return make_float(pow(need_number(args[0], loc, "pow() base"),
                need_number(args[1], loc, "pow() exponent")));
}

static Value bi_minmax(Value *args, int argc, Loc loc, bool min) {
    need_args(min ? "min" : "max", argc, 2, 2, loc);
    double x = need_number(args[0], loc, "value");
    double y = need_number(args[1], loc, "value");
    bool ints = args[0].type == T_INT && args[1].type == T_INT;
    double result = min ? (x < y ? x : y) : (x > y ? x : y);
    if (ints) {
        return make_int(i64_from_double(result));
    }
    return make_float(result);
}

static Value bi_min(Value *args, int argc, Loc loc) {
    return bi_minmax(args, argc, loc, true);
}

static Value bi_max(Value *args, int argc, Loc loc) {
    return bi_minmax(args, argc, loc, false);
}

static uint64_t rng_next(void) {
    g_rng ^= g_rng >> 12;
    g_rng ^= g_rng << 25;
    g_rng ^= g_rng >> 27;
    return g_rng * 0x2545F4914F6CDD1DULL;
}

static Value bi_random(Value *args, int argc, Loc loc) {
    (void)args;
    need_args("random", argc, 0, 0, loc);
    double value = (double)(rng_next() >> 11) / 9007199254740992.0;
    return make_float(value);
}

static Value bi_random_int(Value *args, int argc, Loc loc) {
    need_args("randomInt", argc, 2, 2, loc);
    int64_t from = need_int(args[0], loc, "randomInt min");
    int64_t to = need_int(args[1], loc, "randomInt max");
    if (from > to) {
        int64_t tmp = from;
        from = to;
        to = tmp;
    }
    uint64_t span = (uint64_t)to - (uint64_t)from;
    if (span == UINT64_MAX) {
        return make_int((int64_t)rng_next());
    }
    return make_int(from + (int64_t)(rng_next() % (span + 1)));
}

static Value bi_now(Value *args, int argc, Loc loc) {
    (void)args;
    need_args("now", argc, 0, 0, loc);
    struct timespec ts;
    timespec_get(&ts, TIME_UTC);
    return make_int((int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000);
}

static Value bi_uuid(Value *args, int argc, Loc loc) {
    (void)args;
    need_args("uuid", argc, 0, 0, loc);
    unsigned char bytes[16];
    for (int i = 0; i < 16; i++) {
        bytes[i] = (unsigned char)(rng_next() & 0xFF);
    }
    bytes[6] = (unsigned char)((bytes[6] & 0x0F) | 0x40);
    bytes[8] = (unsigned char)((bytes[8] & 0x3F) | 0x80);
    char text[37];
    snprintf(text, sizeof(text),
            "%02x%02x%02x%02x-%02x%02x-%02x%02x-%02x%02x-%02x%02x%02x%02x%02x%02x",
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5],
            bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11],
            bytes[12], bytes[13], bytes[14], bytes[15]);
    return rt_str(text, 36);
}

static Value bi_print(Value *args, int argc, Loc loc) {
    (void)loc;
    Buf buf;
    buf_init(&buf);
    for (int i = 0; i < argc; i++) {
        if (i > 0) {
            buf_putc(&buf, ' ');
        }
        stringify_into(&buf, args[i], 0);
    }
    buf_putc(&buf, '\n');
    if (buf.len > 0) {
        fwrite(buf.data, 1, buf.len, stdout);
    }
    free(buf.data);
    return make_null();
}

static const Native builtins[] = {
    {"str", 1, 1, bi_str},
    {"toString", 1, 1, bi_str},
    {"num", 1, 1, bi_num},
    {"int", 1, 1, bi_int},
    {"bool", 1, 1, bi_bool},
    {"typeof", 1, 1, bi_typeof},
    {"isNull", 1, 1, bi_is_null},
    {"len", 1, 1, bi_len},
    {"size", 1, 1, bi_len},
    {"list", 0, -1, bi_list},
    {"keys", 1, 1, bi_keys},
    {"values", 1, 1, bi_values},
    {"contains", 2, 2, bi_contains},
    {"add", 2, 2, bi_add},
    {"remove", 2, 2, bi_remove},
    {"sort", 1, 1, bi_sort},
    {"reverse", 1, 1, bi_reverse},
    {"join", 2, 2, bi_join},
    {"split", 2, 2, bi_split},
    {"range", 2, 2, bi_range},
    {"upper", 1, 1, bi_upper},
    {"lower", 1, 1, bi_lower},
    {"trim", 1, 1, bi_trim},
    {"replace", 3, 3, bi_replace},
    {"startsWith", 2, 2, bi_starts_with},
    {"endsWith", 2, 2, bi_ends_with},
    {"substring", 2, 3, bi_substring},
    {"color", 1, 1, bi_color},
    {"colorize", 1, 1, bi_color},
    {"abs", 1, 1, bi_abs},
    {"floor", 1, 1, bi_floor},
    {"ceil", 1, 1, bi_ceil},
    {"round", 1, 1, bi_round},
    {"sqrt", 1, 1, bi_sqrt},
    {"pow", 2, 2, bi_pow},
    {"min", 2, 2, bi_min},
    {"max", 2, 2, bi_max},
    {"random", 0, 0, bi_random},
    {"randomInt", 2, 2, bi_random_int},
    {"now", 0, 0, bi_now},
    {"uuid", 0, 0, bi_uuid},
    {"print", 0, -1, bi_print},
    {"log", 0, -1, bi_print},
};

Value rt_builtin(const char *name) {
    for (size_t i = 0; i < sizeof(builtins) / sizeof(builtins[0]); i++) {
        if (strcmp(builtins[i].name, name) == 0) {
            return make_builtin(&builtins[i]);
        }
    }
    rt_fail((Loc){0, 0}, "internal error: unknown builtin '%s'", name);
}

void rt_init(const char *file) {
    g_file = file;
    struct timespec ts;
    timespec_get(&ts, TIME_UTC);
    g_rng ^= (uint64_t)ts.tv_sec * 1000000007ULL
        ^ (uint64_t)ts.tv_nsec
        ^ (uint64_t)clock();
    if (g_rng == 0) {
        g_rng = 0x9e3779b97f4a7c15ULL;
    }
}
