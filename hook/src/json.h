// Minimal JSON reader and string escaper for the host-to-hook pipe protocol.
// Objects keep insertion order; numbers are doubles; strings are UTF-8.
#pragma once
#include <cstdint>
#include <string>
#include <utility>
#include <vector>

namespace cl {

struct Json {
    enum Type { Null, Bool, Num, Str, Arr, Obj };
    Type t = Null;
    bool b = false;
    double n = 0;
    std::string s;
    std::vector<Json> a;
    std::vector<std::pair<std::string, Json>> o;

    const Json* get(const char* key) const {
        if (t != Obj) return nullptr;
        for (const auto& kv : o)
            if (kv.first == key) return &kv.second;
        return nullptr;
    }
    bool isStr() const { return t == Str; }
    bool isNum() const { return t == Num; }
    bool isArr() const { return t == Arr; }
    bool isObj() const { return t == Obj; }
    bool asBool(bool def) const { return t == Bool ? b : def; }
    int64_t asInt(int64_t def) const { return t == Num ? static_cast<int64_t>(n) : def; }
    // Sets (or replaces) a member; turns a non-object into an empty object first.
    void set(const char* key, Json v) {
        if (t != Obj) {
            *this = Json();
            t = Obj;
        }
        for (auto& kv : o)
            if (kv.first == key) {
                kv.second = std::move(v);
                return;
            }
        o.emplace_back(key, std::move(v));
    }
    void erase(const char* key) {
        for (size_t i = 0; i < o.size(); ++i)
            if (o[i].first == key) {
                o.erase(o.begin() + static_cast<long>(i));
                return;
            }
    }
    static Json number(double v) {
        Json j;
        j.t = Num;
        j.n = v;
        return j;
    }
    static Json string(const std::string& v) {
        Json j;
        j.t = Str;
        j.s = v;
        return j;
    }
    static Json boolean(bool v) {
        Json j;
        j.t = Bool;
        j.b = v;
        return j;
    }
};

bool ParseJson(const std::string& text, Json& out, std::string& err);
std::string JsonEscape(const std::string& s);
// Compact JSON text of a value (integral numbers are printed without a fraction).
std::string JsonWrite(const Json& j);

}  // namespace cl
