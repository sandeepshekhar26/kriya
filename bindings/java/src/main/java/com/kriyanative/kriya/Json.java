package com.kriyanative.kriya;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * A tiny, dependency-free JSON codec. Java SE has no built-in JSON, and the kriya bindings ship with
 * zero runtime dependencies (the Python/.NET bindings use their platform's built-in JSON). Parses a
 * document into plain Java values — {@link Map}&lt;String,Object&gt; / {@link List}&lt;Object&gt; /
 * String / Long / Double / Boolean / null — and serialises them back. Sufficient and faithful for the
 * kriya-host stdio wire protocol and MCP tool schemas. Object key order is preserved (LinkedHashMap).
 */
public final class Json {
    private Json() {}

    /** Thrown on malformed JSON or a non-serialisable value. */
    public static final class JsonException extends RuntimeException {
        public JsonException(String message) { super(message); }
    }

    /** Parse a JSON document into Map / List / String / Long / Double / Boolean / null. */
    public static Object parse(String text) {
        Parser p = new Parser(text);
        p.skipWs();
        Object v = p.value();
        p.skipWs();
        if (!p.atEnd()) throw new JsonException("trailing characters at offset " + p.pos);
        return v;
    }

    /** Parse a JSON object (throws if the top-level value isn't an object). */
    @SuppressWarnings("unchecked")
    public static Map<String, Object> parseObject(String text) {
        Object v = parse(text);
        if (!(v instanceof Map)) throw new JsonException("expected a JSON object");
        return (Map<String, Object>) v;
    }

    /** Serialise a value tree to a compact JSON string. */
    public static String stringify(Object value) {
        StringBuilder sb = new StringBuilder();
        write(value, sb);
        return sb.toString();
    }

    // ── serialise ───────────────────────────────────────────────────────────
    private static void write(Object v, StringBuilder sb) {
        if (v == null) {
            sb.append("null");
        } else if (v instanceof String) {
            writeString((String) v, sb);
        } else if (v instanceof Boolean) {
            sb.append(((Boolean) v) ? "true" : "false");
        } else if (v instanceof Double || v instanceof Float) {
            double d = ((Number) v).doubleValue();
            if (Double.isNaN(d) || Double.isInfinite(d)) throw new JsonException("non-finite number");
            // Render an integral double without a trailing ".0" (e.g. 2.0 -> "2"); JSON-equivalent and
            // keeps the wire clean. Fractional values use the shortest round-trippable form.
            if (d == Math.rint(d) && Math.abs(d) < 1e15) {
                sb.append(Long.toString((long) d));
            } else {
                sb.append(Double.toString(d));
            }
        } else if (v instanceof Number) {
            sb.append(v.toString()); // Long, Integer, Short, Byte, BigInteger…
        } else if (v instanceof Map) {
            sb.append('{');
            boolean first = true;
            for (Map.Entry<?, ?> e : ((Map<?, ?>) v).entrySet()) {
                if (!first) sb.append(',');
                first = false;
                writeString(String.valueOf(e.getKey()), sb);
                sb.append(':');
                write(e.getValue(), sb);
            }
            sb.append('}');
        } else if (v instanceof Iterable) {
            sb.append('[');
            boolean first = true;
            for (Object e : (Iterable<?>) v) {
                if (!first) sb.append(',');
                first = false;
                write(e, sb);
            }
            sb.append(']');
        } else {
            throw new JsonException("cannot serialise " + v.getClass().getName());
        }
    }

    private static void writeString(String s, StringBuilder sb) {
        sb.append('"');
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '"': sb.append("\\\""); break;
                case '\\': sb.append("\\\\"); break;
                case '\b': sb.append("\\b"); break;
                case '\f': sb.append("\\f"); break;
                case '\n': sb.append("\\n"); break;
                case '\r': sb.append("\\r"); break;
                case '\t': sb.append("\\t"); break;
                default:
                    if (c < 0x20) {
                        sb.append(String.format("\\u%04x", (int) c));
                    } else {
                        sb.append(c);
                    }
            }
        }
        sb.append('"');
    }

    // ── parse ───────────────────────────────────────────────────────────────
    private static final class Parser {
        final String s;
        int pos;

        Parser(String s) { this.s = s; }

        boolean atEnd() { return pos >= s.length(); }

        void skipWs() {
            while (pos < s.length()) {
                char c = s.charAt(pos);
                if (c == ' ' || c == '\t' || c == '\n' || c == '\r') pos++;
                else break;
            }
        }

        char peek() {
            if (pos >= s.length()) throw new JsonException("unexpected end of input");
            return s.charAt(pos);
        }

        Object value() {
            char c = peek();
            switch (c) {
                case '{': return object();
                case '[': return array();
                case '"': return string();
                case 't': case 'f': return bool();
                case 'n': literal("null"); return null;
                default: return number();
            }
        }

        Map<String, Object> object() {
            expect('{');
            Map<String, Object> m = new LinkedHashMap<>();
            skipWs();
            if (peek() == '}') { pos++; return m; }
            while (true) {
                skipWs();
                if (peek() != '"') throw new JsonException("expected object key at offset " + pos);
                String key = string();
                skipWs();
                expect(':');
                skipWs();
                m.put(key, value());
                skipWs();
                char c = peek();
                if (c == ',') { pos++; continue; }
                if (c == '}') { pos++; break; }
                throw new JsonException("expected ',' or '}' at offset " + pos);
            }
            return m;
        }

        List<Object> array() {
            expect('[');
            List<Object> a = new ArrayList<>();
            skipWs();
            if (peek() == ']') { pos++; return a; }
            while (true) {
                skipWs();
                a.add(value());
                skipWs();
                char c = peek();
                if (c == ',') { pos++; continue; }
                if (c == ']') { pos++; break; }
                throw new JsonException("expected ',' or ']' at offset " + pos);
            }
            return a;
        }

        String string() {
            expect('"');
            StringBuilder sb = new StringBuilder();
            while (true) {
                if (pos >= s.length()) throw new JsonException("unterminated string");
                char c = s.charAt(pos++);
                if (c == '"') break;
                if (c == '\\') {
                    if (pos >= s.length()) throw new JsonException("unterminated escape");
                    char e = s.charAt(pos++);
                    switch (e) {
                        case '"': sb.append('"'); break;
                        case '\\': sb.append('\\'); break;
                        case '/': sb.append('/'); break;
                        case 'b': sb.append('\b'); break;
                        case 'f': sb.append('\f'); break;
                        case 'n': sb.append('\n'); break;
                        case 'r': sb.append('\r'); break;
                        case 't': sb.append('\t'); break;
                        case 'u':
                            if (pos + 4 > s.length()) throw new JsonException("bad \\u escape");
                            sb.append((char) Integer.parseInt(s.substring(pos, pos + 4), 16));
                            pos += 4;
                            break;
                        default: throw new JsonException("invalid escape \\" + e);
                    }
                } else {
                    sb.append(c);
                }
            }
            return sb.toString();
        }

        Object number() {
            int start = pos;
            boolean isDouble = false;
            if (peek() == '-') pos++;
            while (pos < s.length()) {
                char c = s.charAt(pos);
                if (c >= '0' && c <= '9') { pos++; }
                else if (c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-') { isDouble = true; pos++; }
                else break;
            }
            String num = s.substring(start, pos);
            if (num.isEmpty() || num.equals("-")) throw new JsonException("invalid number at offset " + start);
            if (!isDouble) {
                try { return Long.parseLong(num); }
                catch (NumberFormatException ex) { /* overflow → fall through to double */ }
            }
            try { return Double.parseDouble(num); }
            catch (NumberFormatException ex) { throw new JsonException("invalid number \"" + num + "\""); }
        }

        Boolean bool() {
            if (peek() == 't') { literal("true"); return Boolean.TRUE; }
            literal("false");
            return Boolean.FALSE;
        }

        void literal(String lit) {
            if (pos + lit.length() > s.length() || !s.regionMatches(pos, lit, 0, lit.length()))
                throw new JsonException("expected '" + lit + "' at offset " + pos);
            pos += lit.length();
        }

        void expect(char c) {
            if (pos >= s.length() || s.charAt(pos) != c)
                throw new JsonException("expected '" + c + "' at offset " + pos);
            pos++;
        }
    }
}
