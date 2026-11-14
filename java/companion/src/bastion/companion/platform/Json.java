package bastion.companion.platform;

import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * A strict, bounded JSON reader for the launcher's state messages (RFC 8259; one value, no
 * comments, no trailing commas). Objects become {@code Map<String, Object>} (in document
 * order; a repeated key keeps its last value), arrays {@code List<Object>}, strings
 * {@code String}, numbers {@code Double} (or {@code Long} when integral and exact), and
 * {@code true}/{@code false}/{@code null} {@code Boolean}/{@code null}. Everything returned is
 * unmodifiable.
 *
 * Bounded whatever the input: at most {@link #MAX_CHARS} characters and nesting
 * {@link #MAX_DEPTH} deep; anything else is a {@link Malformed}, never a different exception
 * or a stack overflow. The accessors ({@link #string}, {@link #number}, …) read fields
 * without casts and give a fallback where a field is missing or of another type.
 */
public final class Json {
    /** One state message is at most 64 KiB of UTF-8; no more characters than that. */
    public static final int MAX_CHARS = 64 * 1024;
    public static final int MAX_DEPTH = 32;

    /** The input is not JSON within the limits. */
    public static final class Malformed extends Exception {
        private static final long serialVersionUID = 1L;

        Malformed(String why, int at) {
            super(why + " at character " + at);
        }
    }

    private final String text;
    private int at;

    private Json(String text) {
        this.text = text;
    }

    /** Parses one JSON value. */
    public static Object parse(String text) throws Malformed {
        if (text == null) {
            throw new Malformed("no text", 0);
        }
        if (text.length() > MAX_CHARS) {
            throw new Malformed("too long", MAX_CHARS);
        }
        Json json = new Json(text);
        json.space();
        Object value = json.value(0);
        json.space();
        if (json.at != text.length()) {
            throw new Malformed("trailing characters", json.at);
        }
        return value;
    }

    /** Parses a JSON object (anything else is {@link Malformed}). */
    @SuppressWarnings("unchecked")
    public static Map<String, Object> object(String text) throws Malformed {
        Object value = parse(text);
        if (!(value instanceof Map)) {
            throw new Malformed("not an object", 0);
        }
        return (Map<String, Object>) value;
    }

    private void space() {
        while (at < text.length()) {
            char c = text.charAt(at);
            if (c != ' ' && c != '\t' && c != '\n' && c != '\r') {
                return;
            }
            at++;
        }
    }

    private char peek() throws Malformed {
        if (at >= text.length()) {
            throw new Malformed("unexpected end", at);
        }
        return text.charAt(at);
    }

    private void expect(String word) throws Malformed {
        if (!text.startsWith(word, at)) {
            throw new Malformed("expected " + word, at);
        }
        at += word.length();
    }

    private Object value(int depth) throws Malformed {
        if (depth > MAX_DEPTH) {
            throw new Malformed("nested too deeply", at);
        }
        char c = peek();
        switch (c) {
            case '{':
                return object(depth);
            case '[':
                return array(depth);
            case '"':
                return string();
            case 't':
                expect("true");
                return Boolean.TRUE;
            case 'f':
                expect("false");
                return Boolean.FALSE;
            case 'n':
                expect("null");
                return null;
            default:
                if (c == '-' || (c >= '0' && c <= '9')) {
                    return number();
                }
                throw new Malformed("unexpected character", at);
        }
    }

    private Map<String, Object> object(int depth) throws Malformed {
        at++; // {
        Map<String, Object> map = new LinkedHashMap<>();
        space();
        if (peek() == '}') {
            at++;
            return Collections.unmodifiableMap(map);
        }
        while (true) {
            space();
            if (peek() != '"') {
                throw new Malformed("expected a key", at);
            }
            String key = string();
            space();
            if (peek() != ':') {
                throw new Malformed("expected ':'", at);
            }
            at++;
            space();
            map.put(key, value(depth + 1));
            space();
            char next = peek();
            at++;
            if (next == '}') {
                return Collections.unmodifiableMap(map);
            }
            if (next != ',') {
                throw new Malformed("expected ',' or '}'", at - 1);
            }
        }
    }

    private List<Object> array(int depth) throws Malformed {
        at++; // [
        List<Object> list = new ArrayList<>();
        space();
        if (peek() == ']') {
            at++;
            return Collections.unmodifiableList(list);
        }
        while (true) {
            space();
            list.add(value(depth + 1));
            space();
            char next = peek();
            at++;
            if (next == ']') {
                return Collections.unmodifiableList(list);
            }
            if (next != ',') {
                throw new Malformed("expected ',' or ']'", at - 1);
            }
        }
    }

    private String string() throws Malformed {
        at++; // "
        StringBuilder out = new StringBuilder();
        while (true) {
            char c = peek();
            at++;
            if (c == '"') {
                return out.toString();
            }
            if (c < 0x20) {
                throw new Malformed("control character in a string", at - 1);
            }
            if (c != '\\') {
                out.append(c);
                continue;
            }
            char escape = peek();
            at++;
            switch (escape) {
                case '"': out.append('"'); break;
                case '\\': out.append('\\'); break;
                case '/': out.append('/'); break;
                case 'b': out.append('\b'); break;
                case 'f': out.append('\f'); break;
                case 'n': out.append('\n'); break;
                case 'r': out.append('\r'); break;
                case 't': out.append('\t'); break;
                case 'u': {
                    if (at + 4 > text.length()) {
                        throw new Malformed("short \\u escape", at);
                    }
                    int code = 0;
                    for (int i = 0; i < 4; i++) {
                        int digit = Character.digit(text.charAt(at + i), 16);
                        if (digit < 0) {
                            throw new Malformed("bad \\u escape", at + i);
                        }
                        code = code * 16 + digit;
                    }
                    at += 4;
                    out.append((char) code);
                    break;
                }
                default:
                    throw new Malformed("bad escape", at - 1);
            }
        }
    }

    private Object number() throws Malformed {
        int start = at;
        if (peek() == '-') {
            at++;
        }
        char first = peek();
        if (first == '0') {
            at++;
        } else if (first >= '1' && first <= '9') {
            while (at < text.length() && text.charAt(at) >= '0' && text.charAt(at) <= '9') {
                at++;
            }
        } else {
            throw new Malformed("bad number", at);
        }
        boolean integral = true;
        if (at < text.length() && text.charAt(at) == '.') {
            integral = false;
            at++;
            digits();
        }
        if (at < text.length() && (text.charAt(at) == 'e' || text.charAt(at) == 'E')) {
            integral = false;
            at++;
            if (at < text.length() && (text.charAt(at) == '+' || text.charAt(at) == '-')) {
                at++;
            }
            digits();
        }
        String literal = text.substring(start, at);
        if (integral && literal.length() <= 18) {
            return Long.parseLong(literal);
        }
        double value = Double.parseDouble(literal);
        if (Double.isInfinite(value)) {
            throw new Malformed("number out of range", start);
        }
        return value;
    }

    private void digits() throws Malformed {
        int start = at;
        while (at < text.length() && text.charAt(at) >= '0' && text.charAt(at) <= '9') {
            at++;
        }
        if (at == start) {
            throw new Malformed("expected digits", at);
        }
    }

    // ------------------------------------------------------------------ reading fields

    /** {@code object.get(key)} if it is a string, else {@code fallback}. */
    public static String string(Map<String, Object> object, String key, String fallback) {
        Object value = object == null ? null : object.get(key);
        return value instanceof String ? (String) value : fallback;
    }

    /** {@code object.get(key)} if it is a number, else {@code fallback}. */
    public static double number(Map<String, Object> object, String key, double fallback) {
        Object value = object == null ? null : object.get(key);
        return value instanceof Number ? ((Number) value).doubleValue() : fallback;
    }

    /** {@code object.get(key)} as a long if it is a number (rounded toward zero), else {@code fallback}. */
    public static long integer(Map<String, Object> object, String key, long fallback) {
        Object value = object == null ? null : object.get(key);
        return value instanceof Number ? ((Number) value).longValue() : fallback;
    }

    /** {@code object.get(key)} if it is a boolean, else {@code fallback}. */
    public static boolean flag(Map<String, Object> object, String key, boolean fallback) {
        Object value = object == null ? null : object.get(key);
        return value instanceof Boolean ? (Boolean) value : fallback;
    }

    /** {@code object.get(key)} if it is an object, else null. */
    @SuppressWarnings("unchecked")
    public static Map<String, Object> child(Map<String, Object> object, String key) {
        Object value = object == null ? null : object.get(key);
        return value instanceof Map ? (Map<String, Object>) value : null;
    }

    /** {@code object.get(key)} if it is an array, else an empty list. */
    @SuppressWarnings("unchecked")
    public static List<Object> list(Map<String, Object> object, String key) {
        Object value = object == null ? null : object.get(key);
        return value instanceof List ? (List<Object>) value : Collections.emptyList();
    }

    /** The objects among {@code list}'s elements (others are skipped). */
    @SuppressWarnings("unchecked")
    public static List<Map<String, Object>> objects(List<Object> list) {
        List<Map<String, Object>> out = new ArrayList<>();
        for (Object item : list) {
            if (item instanceof Map) {
                out.add((Map<String, Object>) item);
            }
        }
        return out;
    }
}
