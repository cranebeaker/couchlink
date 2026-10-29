#include "json.h"

#include <windows.h>

#include <cstdio>
#include <cstdlib>

namespace cl {
namespace {

struct Parser {
    const std::string& src;
    size_t i = 0;
    std::string err;
    explicit Parser(const std::string& s) : src(s) {}

    bool fail(const char* msg) {
        if (err.empty()) err = msg;
        return false;
    }
    void ws() {
        while (i < src.size() && (src[i] == ' ' || src[i] == '\t' || src[i] == '\r' || src[i] == '\n')) ++i;
    }
    bool lit(const char* word) {
        size_t k = 0;
        while (word[k]) {
            if (i + k >= src.size() || src[i + k] != word[k]) return false;
            ++k;
        }
        i += k;
        return true;
    }
    static void utf8(std::string& out, uint32_t cp) {
        if (cp < 0x80) {
            out += static_cast<char>(cp);
        } else if (cp < 0x800) {
            out += static_cast<char>(0xC0 | (cp >> 6));
            out += static_cast<char>(0x80 | (cp & 0x3F));
        } else if (cp < 0x10000) {
            out += static_cast<char>(0xE0 | (cp >> 12));
            out += static_cast<char>(0x80 | ((cp >> 6) & 0x3F));
            out += static_cast<char>(0x80 | (cp & 0x3F));
        } else {
            out += static_cast<char>(0xF0 | (cp >> 18));
            out += static_cast<char>(0x80 | ((cp >> 12) & 0x3F));
            out += static_cast<char>(0x80 | ((cp >> 6) & 0x3F));
            out += static_cast<char>(0x80 | (cp & 0x3F));
        }
    }
    bool hex4(uint32_t& v) {
        if (i + 4 > src.size()) return fail("short unicode escape");
        v = 0;
        for (int k = 0; k < 4; ++k) {
            char c = src[i++];
            v <<= 4;
            if (c >= '0' && c <= '9') v |= static_cast<uint32_t>(c - '0');
            else if (c >= 'a' && c <= 'f') v |= static_cast<uint32_t>(c - 'a' + 10);
            else if (c >= 'A' && c <= 'F') v |= static_cast<uint32_t>(c - 'A' + 10);
            else return fail("bad unicode escape");
        }
        return true;
    }
    bool str(std::string& out) {
        if (i >= src.size() || src[i] != '"') return fail("expected string");
        ++i;
        while (i < src.size()) {
            char c = src[i++];
            if (c == '"') return true;
            if (c != '\\') {
                out += c;
                continue;
            }
            if (i >= src.size()) break;
            char e = src[i++];
            switch (e) {
                case '"': out += '"'; break;
                case '\\': out += '\\'; break;
                case '/': out += '/'; break;
                case 'b': out += '\b'; break;
                case 'f': out += '\f'; break;
                case 'n': out += '\n'; break;
                case 'r': out += '\r'; break;
                case 't': out += '\t'; break;
                case 'u': {
                    uint32_t cp;
                    if (!hex4(cp)) return false;
                    if (cp >= 0xD800 && cp < 0xDC00 && i + 1 < src.size() && src[i] == '\\' && src[i + 1] == 'u') {
                        i += 2;
                        uint32_t lo;
                        if (!hex4(lo)) return false;
                        cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                    }
                    utf8(out, cp);
                    break;
                }
                default: return fail("bad escape");
            }
        }
        return fail("unterminated string");
    }
    bool value(Json& v, int depth) {
        if (depth > 64) return fail("too deep");
        ws();
        if (i >= src.size()) return fail("unexpected end");
        char c = src[i];
        if (c == '{') {
            ++i;
            v.t = Json::Obj;
            ws();
            if (i < src.size() && src[i] == '}') {
                ++i;
                return true;
            }
            for (;;) {
                ws();
                std::string key;
                if (!str(key)) return false;
                ws();
                if (i >= src.size() || src[i] != ':') return fail("expected colon");
                ++i;
                Json child;
                if (!value(child, depth + 1)) return false;
                v.o.emplace_back(std::move(key), std::move(child));
                ws();
                if (i < src.size() && src[i] == ',') {
                    ++i;
                    continue;
                }
                if (i < src.size() && src[i] == '}') {
                    ++i;
                    return true;
                }
                return fail("expected comma or closing brace");
            }
        }
        if (c == '[') {
            ++i;
            v.t = Json::Arr;
            ws();
            if (i < src.size() && src[i] == ']') {
                ++i;
                return true;
            }
            for (;;) {
                Json child;
                if (!value(child, depth + 1)) return false;
                v.a.push_back(std::move(child));
                ws();
                if (i < src.size() && src[i] == ',') {
                    ++i;
                    continue;
                }
                if (i < src.size() && src[i] == ']') {
                    ++i;
                    return true;
                }
                return fail("expected comma or closing bracket");
            }
        }
        if (c == '"') {
            v.t = Json::Str;
            return str(v.s);
        }
        if (lit("true")) {
            v.t = Json::Bool;
            v.b = true;
            return true;
        }
        if (lit("false")) {
            v.t = Json::Bool;
            v.b = false;
            return true;
        }
        if (lit("null")) {
            v.t = Json::Null;
            return true;
        }
        char* end = nullptr;
        double d = std::strtod(src.c_str() + i, &end);
        if (end == src.c_str() + i) return fail("unexpected character");
        i = static_cast<size_t>(end - src.c_str());
        v.t = Json::Num;
        v.n = d;
        return true;
    }
};

}  // namespace

bool ParseJson(const std::string& text, Json& out, std::string& err) {
    Parser p(text);
    if (!p.value(out, 0)) {
        err = p.err;
        return false;
    }
    p.ws();
    if (p.i != text.size()) {
        err = "trailing characters";
        return false;
    }
    return true;
}

std::string JsonEscape(const std::string& s) {
    std::string r;
    r.reserve(s.size() + 2);
    for (unsigned char c : s) {
        switch (c) {
            case '"': r += "\\\""; break;
            case '\\': r += "\\\\"; break;
            case '\n': r += "\\n"; break;
            case '\r': r += "\\r"; break;
            case '\t': r += "\\t"; break;
            default:
                if (c < 0x20) {
                    char buf[8];
                    wsprintfA(buf, "\\u%04x", c);
                    r += buf;
                } else {
                    r += static_cast<char>(c);
                }
        }
    }
    return r;
}

std::string JsonWrite(const Json& j) {
    switch (j.t) {
        case Json::Null: return "null";
        case Json::Bool: return j.b ? "true" : "false";
        case Json::Num: {
            char buf[64];
            if (j.n == static_cast<double>(static_cast<long long>(j.n))) snprintf(buf, sizeof(buf), "%lld", static_cast<long long>(j.n));
            else snprintf(buf, sizeof(buf), "%.17g", j.n);
            return buf;
        }
        case Json::Str: return "\"" + JsonEscape(j.s) + "\"";
        case Json::Arr: {
            std::string r = "[";
            for (size_t i = 0; i < j.a.size(); ++i) {
                if (i) r += ",";
                r += JsonWrite(j.a[i]);
            }
            return r + "]";
        }
        case Json::Obj: {
            std::string r = "{";
            for (size_t i = 0; i < j.o.size(); ++i) {
                if (i) r += ",";
                r += "\"" + JsonEscape(j.o[i].first) + "\":" + JsonWrite(j.o[i].second);
            }
            return r + "}";
        }
    }
    return "null";
}

}  // namespace cl
