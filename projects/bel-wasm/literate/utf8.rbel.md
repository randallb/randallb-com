# Unicode text and byte lists

A character is a Unicode scalar. A string is a list of characters. The existing
native filter interface transports bytes as character values 0–255; that is a
byte representation, not an implicit text encoding. Include `integers.rbel.md`
with this chapter. `utf8-encode` produces UTF-8 bytes; `utf8-decode` requires a
complete, valid UTF-8 byte sequence. There is no implicit normalization or case
folding. An arbitrary input chunk may end in the middle of a UTF-8 sequence.

Both functions reject improper/cyclic input lists. Encoding rejects noncharacters;
decoding rejects nonbytes, incomplete/overlong sequences, surrogate encodings,
and scalars above U+10FFFF. Errors are ordinary catchable language errors.

```rbel definition=utf8-list-boundary
(def u-reverse (xs out)
  (if xs (u-reverse (cdr xs) (join (car xs) out)) out))
(def u-prepend-reversed (xs out)
  (if xs (u-prepend-reversed (cdr xs) (join (car xs) out)) out))
(def u-proper-step (fast slow)
  (if fast
    (if (id (type fast) 'pair)
      (if (cdr fast)
        (if (id (type (cdr fast)) 'pair)
          ((fn (next after)
             (if (id next after) (err 'expected-finite-list)
               (u-proper-step next after)))
           (cdr (cdr fast)) (cdr slow))
          (err 'expected-finite-list)) t)
      (err 'expected-finite-list)) t))
(def u-require-list (xs)
  (if (u-proper-step xs xs) xs (err 'expected-finite-list)))
(def u-div (a b)
  (n-raw '+ (n-quotient (n-numerator a) (n-numerator b)) "1"))
(def u-mod (a b)
  (n-raw '+ (n-remainder (n-numerator a) (n-numerator b)) "1"))
(def u-byte (c)
  ((fn (value) (if (< value 256) value (err 'expected-byte))) (char-code c)))
```

```rbel definition=utf8-encoding
(def u-two-bytes (a b) (join (code-char a) (join (code-char b) nil)))
(def u-three-bytes (a b c)
  (join (code-char a) (u-two-bytes b c)))
(def u-four-bytes (a b c d)
  (join (code-char a) (u-three-bytes b c d)))
(def u-encode-scalar (c)
  (if (< c 128) (join (code-char c) nil)
    (if (< c 2048)
      (u-two-bytes (+ 192 (u-div c 64)) (+ 128 (u-mod c 64)))
      (if (< c 65536)
        (u-three-bytes (+ 224 (u-div c 4096))
          (+ 128 (u-mod (u-div c 64) 64)) (+ 128 (u-mod c 64)))
        (u-four-bytes (+ 240 (u-div c 262144))
          (+ 128 (u-mod (u-div c 4096) 64))
          (+ 128 (u-mod (u-div c 64) 64)) (+ 128 (u-mod c 64)))))))
(def u-encode-loop (text out)
  (if text
    (u-encode-loop (cdr text)
      (u-prepend-reversed (u-encode-scalar (char-code (car text))) out))
    (u-reverse out nil)))
(def utf8-encode (text) (u-encode-loop (u-require-list text) nil))
```

```rbel definition=utf8-decoding
(def u-decode-continuations (bytes remaining scalar minimum out)
  (if (= remaining 0)
    (if (< scalar minimum) (err 'invalid-utf8)
      (u-decode-loop bytes (join (code-char scalar) out)))
    (if bytes
      ((fn (byte)
         (if (if (>= byte 128) (< byte 192) nil)
           (u-decode-continuations (cdr bytes) (- remaining 1)
             (+ (* scalar 64) (- byte 128)) minimum out)
           (err 'invalid-utf8)))
       (u-byte (car bytes)))
      (err 'truncated-utf8))))
(def u-decode-leading (bytes first out)
  (if (< first 128) (u-decode-loop bytes (join (code-char first) out))
    (if (if (>= first 194) (< first 224) nil)
      (u-decode-continuations bytes 1 (- first 192) 128 out)
      (if (if (>= first 224) (< first 240) nil)
        (u-decode-continuations bytes 2 (- first 224) 2048 out)
        (if (if (>= first 240) (< first 245) nil)
          (u-decode-continuations bytes 3 (- first 240) 65536 out)
          (err 'invalid-utf8))))))
(def u-decode-loop (bytes out)
  (if bytes (u-decode-leading (cdr bytes) (u-byte (car bytes)) out)
    (u-reverse out nil)))
(def utf8-decode (bytes) (u-decode-loop (u-require-list bytes) nil))
```

```rbel example=utf8-accent-bytes
(utf8-encode "é")
=>
(\u{C3} \u{A9})
```

```rbel example=utf8-accent-text
(utf8-decode '(\u{C3} \u{A9}))
=>
"é"
```

```rbel example=utf8-supplementary-roundtrip
(utf8-decode (utf8-encode "雪🦉"))
=>
"雪🦉"
```

```rbel example=utf8-nul-roundtrip
(utf8-decode (utf8-encode '(\u{0} \u{10FFFF})))
=>
(\u{0} \u{10FFFF})
```

```rbel example=utf8-malformed-is-catchable
(try (utf8-decode '(\u{C0} \u{80})) (catch problem problem))
=>
invalid-utf8
```

```rbel example=utf8-partial-is-catchable
(try (utf8-decode '(\u{F0} \u{9F})) (catch problem problem))
=>
truncated-utf8
```
