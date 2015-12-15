/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use app_units::Au;
use core::nonzero::NonZero;
use cssparser::{self, Color, RGBA};
use js::conversions::{FromJSValConvertible, ToJSValConvertible};
use js::jsapi::{JSContext, JSString, HandleValue, MutableHandleValue};
use js::jsapi::{JS_GetTwoByteStringCharsAndLength, JS_GetLatin1StringCharsAndLength, JS_StringHasLatin1Chars};
use js::jsval::StringValue;
use js::rust::ToString;
use libc::c_char;
use num_lib::ToPrimitive;
use opts;
use serde;
use std::ascii::AsciiExt;
use std::borrow::ToOwned;
use std::char;
use std::cmp::Ordering;
use std::convert::AsRef;
use std::ffi::CStr;
use std::fmt;
use std::intrinsics;
use std::iter::{Filter, Peekable};
use std::mem;
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::slice;
use std::str::{CharIndices, FromStr, Split, from_utf8, from_utf8_unchecked};
use std::hash::{Hash, Hasher};
use string_cache::Atom;

// An unpacked DOM String is either a String on the heap,
// or an atom or an array of bytes. Logically, this is the representation
// of strings, but the unpacked version is 4 words, compared to 3 for a String,

enum UnpackedDOMString<A,B,C> {
    Atomic(A),
    Inlined(B),
    Stringy(C),
}

// A packed DOMString that is 3 words long. This relies on the fact that the first
// word in a String is a word-aligned pointer, so cannot be 3 or 5 (it can be 1 since that is used as heap::EMPTY).
// We can use the first word as a flag saying whether the DOMString is a String or an Atom or an inlined string.
// FIXME(ajeffrey): this relies on DOMString being 192 bits long

const ATOM: u64 = 3;
const INLINED: u64 = 5;

#[unsafe_no_drop_flag]
pub struct DOMString {
    flag: NonZero<u64>,
    data: [u8;16],
}

struct DOMStringAtom {
    flag: NonZero<u64>,
    atom: Atom,
    padding: u64,
}

struct DOMStringInlined {
    flag: NonZero<u64>,
    string: InlinedString,
}

#[derive(Clone,Copy)]
pub struct InlinedString{
    bytes: [u8;15],
    length: u8,
}

impl !Send for DOMString {}

#[inline]
#[allow(mutable_transmutes)]
unsafe fn from_utf8_unchecked_mut(bytes: &mut [u8]) -> &mut str {
    mem::transmute(from_utf8_unchecked(bytes))
}

#[inline]
unsafe fn as_inlined_string(string: &str) -> InlinedString {
    if (string.len() == 0) {
        InlinedString { bytes: [0;15], length: 0 }
    } else {
        // FIXME(ajeffrey): this can fail if the string length < 9,
        // and the string is stored as the last word on a page of memory,
        // and the next page is unreadable.
        assert!(string.len() < 16);
        let transmuted: &InlinedString = unsafe { mem::transmute(string.as_ptr()) };
        let mut result = *transmuted;
        result.length = (string.len() as u8);
        result
    }
}

impl Deref for InlinedString {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        unsafe { from_utf8_unchecked(&self.bytes[0..(self.length as usize)]) }
    }
}

impl DerefMut for InlinedString {
    #[inline]
    fn deref_mut(&mut self) -> &mut str {
        unsafe { from_utf8_unchecked_mut(&mut self.bytes[0..(self.length as usize)]) }
    }
}

impl DOMString {
    #[inline]
    fn unpack(self) -> UnpackedDOMString<Atom, InlinedString, String> {
        match *self.flag {
            ATOM => {
                let transmuted: DOMStringAtom = unsafe { mem::transmute(self) };
                UnpackedDOMString::Atomic(transmuted.atom)
            },
            INLINED => {
                let transmuted: DOMStringInlined = unsafe { mem::transmute(self) };
                UnpackedDOMString::Inlined(transmuted.string)
            },
            _ => {
                let transmuted: String = unsafe { mem::transmute(self) };
                UnpackedDOMString::Stringy(transmuted)
            }
        }
    }
    #[inline]
    fn unpack_ref(&self) -> UnpackedDOMString<&Atom, &InlinedString, &String> {
        match *self.flag {
            ATOM => {
                let transmuted: &DOMStringAtom = unsafe { mem::transmute(self) };
                UnpackedDOMString::Atomic(&transmuted.atom)
            },
            INLINED => {
                let transmuted: &DOMStringInlined = unsafe { mem::transmute(self) };
                UnpackedDOMString::Inlined(&transmuted.string)
            },
            _ => {
                let transmuted: &String = unsafe { mem::transmute(self) };
                UnpackedDOMString::Stringy(transmuted)
            }
        }
    }
    #[inline]
    fn unpack_mut(&mut self) -> UnpackedDOMString<&mut Atom, &mut InlinedString, &mut String> {
        match *self.flag {
            ATOM => {
                let transmuted: &mut DOMStringAtom = unsafe { mem::transmute(self) };
                UnpackedDOMString::Atomic(&mut transmuted.atom)
            },
            INLINED => {
                let transmuted: &mut DOMStringInlined = unsafe { mem::transmute(self) };
                UnpackedDOMString::Inlined(&mut transmuted.string)
            },
            _ => {
                let transmuted: &mut String = unsafe { mem::transmute(self) };
                UnpackedDOMString::Stringy(transmuted)
            }
        }
    }
    #[inline]
    pub fn new() -> DOMString {
        DOMString::from(atom!(""))
    }
    #[inline]
    pub fn clear(&mut self) {
        match self.unpack_mut() {
            UnpackedDOMString::Atomic(atom) => {},
            UnpackedDOMString::Inlined(string) => {},
            UnpackedDOMString::Stringy(string) => { return string.clear(); },
        }
        *self = DOMString::new();
    }
    #[inline]
    pub fn push_str(&mut self, contents: &str) {
        let mut string = match self.unpack_mut() {
            UnpackedDOMString::Atomic(atom) => String::from(&**atom),
            UnpackedDOMString::Inlined(string) => String::from(&**string),
            UnpackedDOMString::Stringy(string) => { return string.push_str(contents); },
        };
        string.push_str(contents);
        *self = DOMString::from(string);
    }
}

impl Clone for DOMString {
    fn clone(&self) -> DOMString {
        match self.unpack_ref() {
            UnpackedDOMString::Atomic(atom) => DOMString::from(atom.clone()),
            UnpackedDOMString::Inlined(string) => DOMString::from(string.clone()),
            UnpackedDOMString::Stringy(string) => DOMString::from(string.clone()),
        }
    }
}

impl Drop for DOMString {
    #[inline]
    fn drop(&mut self) {
        if (*self.flag != 0) && (*self.flag != mem::POST_DROP_U64) {
            // We need to make sure that the memory for a String or Atom is reclaimed appropriately.
            // We do this by zeroing the contents.
            match self.unpack_mut() {
                UnpackedDOMString::Atomic(atom) => {
                    unsafe { *atom = mem::zeroed() }
                },
                UnpackedDOMString::Inlined(string) => {},
                UnpackedDOMString::Stringy(string) => {
                    unsafe { *string = mem::zeroed() }
                }
            }
        }
    }
}

impl PartialEq for DOMString {
    #[inline]
    fn eq(&self, other: &DOMString) -> bool {
        (&**self).eq(&**other)
    }
}

impl Eq for DOMString {}

impl Hash for DOMString {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        (&**self).hash(state)
    }
}

impl PartialOrd for DOMString {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        (&**self).partial_cmp(&**other)
    }
}

impl Ord for DOMString {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        (&**self).cmp(&**other)
    }
}

impl fmt::Debug for DOMString {
    #[inline]
    fn fmt(&self, formatter: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        (&**self).fmt(formatter)
    }
}

impl serde::Deserialize for DOMString {
    #[inline]
    fn deserialize<D>(deserializer: &mut D) -> Result<Self, D::Error> where D: serde::Deserializer {
        let string: String = try!(serde::Deserialize::deserialize(deserializer));
        Ok(DOMString::from(string))
    }
}

impl serde::Serialize for DOMString {
    #[inline]
    fn serialize<S>(&self, serializer: &mut S) -> Result<(), S::Error> where S: serde::Serializer {
        (&**self).serialize(serializer)
    }
}

impl Default for DOMString {
    #[inline]
    fn default() -> Self {
       DOMString::new()
    }
}

impl Deref for DOMString {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        match self.unpack_ref() {
            UnpackedDOMString::Atomic(atom) => atom.deref(),
            UnpackedDOMString::Inlined(string) => string.deref(),
            UnpackedDOMString::Stringy(string) => string.deref()
        }
    }
}

// impl DerefMut for DOMString {
//     #[inline]
//     fn deref_mut(&mut self) -> &mut str {
//         if *self.flag == ATOM {
//             *self = DOMString::from(String::from(&**self));
//         }
//         match self.unpack_mut() {
//             UnpackedDOMString::Atomic(atom) => panic!("Mutating atoms."),
//             UnpackedDOMString::Inlined(string) => string.deref_mut(),
//             UnpackedDOMString::Stringy(string) => string.deref_mut(),
//         }
//     }
// }

impl AsRef<str> for DOMString {
    #[inline]
    fn as_ref(&self) -> &str {
        self.deref()
    }
}

impl fmt::Display for DOMString {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl PartialEq<str> for DOMString {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        &**self == other
    }
}

impl<'a> PartialEq<&'a str> for DOMString {
    #[inline]
    fn eq(&self, other: &&'a str) -> bool {
        &**self == *other
    }
}

impl From<String> for DOMString {
    #[inline]
    fn from(contents: String) -> DOMString {
        unsafe { mem::transmute(contents) }
    }
}

impl From<Atom> for DOMString {
    #[inline]
    fn from(atom: Atom) -> DOMString {
        unsafe {
            mem::transmute(DOMStringAtom{
                flag: NonZero::new(ATOM),
                atom: atom,
                padding: 0
            })
        }
    }
}

impl From<InlinedString> for DOMString {
    #[inline]
    fn from(string: InlinedString) -> DOMString {
        unsafe {
            mem::transmute(DOMStringInlined{
                flag: NonZero::new(INLINED),
                string: string,
            })
        }
    }
}

impl<'a> From<&'a str> for DOMString {
    #[inline]
    fn from(string: &str) -> DOMString {
        if (string.len() < 16) {
            DOMString::from(unsafe { as_inlined_string(string) })
        } else {
            DOMString::from(String::from(string))
        }
    }
}

impl From<DOMString> for String {
    #[inline]
    fn from(this: DOMString) -> String {
        match this.unpack() {
            UnpackedDOMString::Atomic(atom) => String::from(&*atom),
            UnpackedDOMString::Inlined(string) => String::from(&*string),
            UnpackedDOMString::Stringy(string) => string,
        }
    }
}

impl From<DOMString> for Atom {
    #[inline]
    fn from(this: DOMString) -> Atom {
        match this.unpack() {
            UnpackedDOMString::Atomic(atom) => atom,
            UnpackedDOMString::Inlined(string) => Atom::from(&*string),
            UnpackedDOMString::Stringy(string) => Atom::from(&*string),
        }
    }
}

impl<'a> From<&'a DOMString> for Atom {
    #[inline]
    fn from(this: &DOMString) -> Atom {
        // FIXME(ajeffrey): unsafely update the DOMString to be an atom?
        match this.unpack_ref() {
            UnpackedDOMString::Atomic(atom) => atom.clone(),
            UnpackedDOMString::Inlined(string) => Atom::from(&**string),
            UnpackedDOMString::Stringy(string) => Atom::from(&**string),
        }
    }
}

impl Into<Vec<u8>> for DOMString {
    #[inline]
    fn into(self) -> Vec<u8> {
        String::from(self).into()
    }
}

impl Extend<char> for DOMString {
    #[inline]
    fn extend<I>(&mut self, iterable: I) where I: IntoIterator<Item=char> {
        let mut string = match self.unpack_mut() {
            UnpackedDOMString::Atomic(atom) => String::from(&**atom),
            UnpackedDOMString::Inlined(string) => String::from(&**string),
            UnpackedDOMString::Stringy(string) => { return string.extend(iterable); },
        };
        string.extend(iterable);
        *self = DOMString::from(string);
    }
}

// https://heycam.github.io/webidl/#es-DOMString
impl ToJSValConvertible for DOMString {
    unsafe fn to_jsval(&self, cx: *mut JSContext, rval: MutableHandleValue) {
        (**self).to_jsval(cx, rval)
    }
}

/// Behavior for stringification of `JSVal`s.
#[derive(PartialEq)]
pub enum StringificationBehavior {
    /// Convert `null` to the string `"null"`.
    Default,
    /// Convert `null` to the empty string.
    Empty,
}

/// Given an iterator and an bounds on the length in utf8 bytes, convert it to a DOMString.
/// Can panic if the bounds are incorrect.
pub unsafe fn iter_to_str<I>(char_iterator: I, length_lower_bound: usize, length_upper_bound: usize) -> DOMString
    where I : Iterator<Item=char>
{
    if length_upper_bound <= 32 {
        let mut index = 0;
        let mut buffer = [0;32];
        for ch in char_iterator { index += ch.encode_utf8(&mut buffer[index..]).unwrap() }
        DOMString::from(from_utf8_unchecked(&buffer[0..index]))
    } else {
        let mut string = String::with_capacity(length_lower_bound);
        string.extend(char_iterator);
        DOMString::from(string)
    }
}

/// Convert the given `JSString` to a `DOMString`. Fails if the string does not
/// contain valid UTF-16.
pub unsafe fn jsstring_to_str(cx: *mut JSContext, s: *mut JSString) -> DOMString {
    if JS_StringHasLatin1Chars(s) {
        let mut latin1_length = 0;
        let latin1_chars = JS_GetLatin1StringCharsAndLength(cx, ptr::null(), s, &mut latin1_length);
        assert!(!latin1_chars.is_null());
        let latin1_slice = slice::from_raw_parts(latin1_chars, latin1_length);
        let char_iterator = latin1_slice.iter().map(|&c| c as char);
        iter_to_str(char_iterator, latin1_length, latin1_length * 2)
    } else {
        let mut two_byte_length = 0;
        let two_byte_chars = JS_GetTwoByteStringCharsAndLength(cx, ptr::null(), s, &mut two_byte_length);
        assert!(!two_byte_chars.is_null());
        let two_byte_slice = slice::from_raw_parts(two_byte_chars, two_byte_length);
        let char_iterator = char::decode_utf16(two_byte_slice.iter().cloned()).map(|item| {
            match item {
                Ok(c) => c,
                Err(_) => {
                    // FIXME: Add more info like document URL in the message?
                    macro_rules! message {
                        () => {
                            "Found an unpaired surrogate in a DOM string. \
                             If you see this in real web content, \
                             please comment on https://github.com/servo/servo/issues/6564"
                        }
                    }
                    if opts::get().replace_surrogates {
                        error!(message!());
                        '\u{FFFD}'
                    } else {
                        panic!(concat!(message!(), " Use `-Z replace-surrogates` \
                            on the command line to make this non-fatal."));
                    }
                }
            }
        });
        iter_to_str(char_iterator, two_byte_length, two_byte_length + (two_byte_length >> 1))
    }
}

// https://heycam.github.io/webidl/#es-DOMString
impl FromJSValConvertible for DOMString {
    type Config = StringificationBehavior;
    unsafe fn from_jsval(cx: *mut JSContext,
                         value: HandleValue,
                         null_behavior: StringificationBehavior)
                         -> Result<DOMString, ()> {
        if null_behavior == StringificationBehavior::Empty &&
           value.get().is_null() {
            Ok(DOMString::new())
        } else {
            let jsstr = ToString(cx, value);
            if jsstr.is_null() {
                debug!("ToString failed");
                Err(())
            } else {
                Ok(jsstring_to_str(cx, jsstr))
            }
        }
    }
}

// impl Extend<char> for DOMString {
//     fn extend<I>(&mut self, iterable: I) where I: IntoIterator<Item=char> {
//         self.0.extend(iterable)
//     }
// }

pub type StaticCharVec = &'static [char];
pub type StaticStringVec = &'static [&'static str];

/// Whitespace as defined by HTML5 § 2.4.1.
// TODO(SimonSapin) Maybe a custom Pattern can be more efficient?
const WHITESPACE: &'static [char] = &[' ', '\t', '\x0a', '\x0c', '\x0d'];

pub fn is_whitespace(s: &str) -> bool {
    s.chars().all(char_is_whitespace)
}

#[inline]
pub fn char_is_whitespace(c: char) -> bool {
    WHITESPACE.contains(&c)
}

/// A "space character" according to:
///
/// https://html.spec.whatwg.org/multipage/#space-character
pub static HTML_SPACE_CHARACTERS: StaticCharVec = &[
    '\u{0020}',
    '\u{0009}',
    '\u{000a}',
    '\u{000c}',
    '\u{000d}',
];

pub fn split_html_space_chars<'a>(s: &'a str) ->
                                  Filter<Split<'a, StaticCharVec>, fn(&&str) -> bool> {
    fn not_empty(&split: &&str) -> bool { !split.is_empty() }
    s.split(HTML_SPACE_CHARACTERS).filter(not_empty as fn(&&str) -> bool)
}


fn is_ascii_digit(c: &char) -> bool {
    match *c {
        '0'...'9' => true,
        _ => false,
    }
}


fn read_numbers<I: Iterator<Item=char>>(mut iter: Peekable<I>) -> Option<i64> {
    match iter.peek() {
        Some(c) if is_ascii_digit(c) => (),
        _ => return None,
    }

    iter.take_while(is_ascii_digit).map(|d| {
        d as i64 - '0' as i64
    }).fold(Some(0i64), |accumulator, d| {
        accumulator.and_then(|accumulator| {
            accumulator.checked_mul(10)
        }).and_then(|accumulator| {
            accumulator.checked_add(d)
        })
    })
}


/// Shared implementation to parse an integer according to
/// <https://html.spec.whatwg.org/multipage/#rules-for-parsing-integers> or
/// <https://html.spec.whatwg.org/multipage/#rules-for-parsing-non-negative-integers>
fn do_parse_integer<T: Iterator<Item=char>>(input: T) -> Option<i64> {
    let mut input = input.skip_while(|c| {
        HTML_SPACE_CHARACTERS.iter().any(|s| s == c)
    }).peekable();

    let sign = match input.peek() {
        None => return None,
        Some(&'-') => {
            input.next();
            -1
        },
        Some(&'+') => {
            input.next();
            1
        },
        Some(_) => 1,
    };

    let value = read_numbers(input);

    value.and_then(|value| value.checked_mul(sign))
}

/// Parse an integer according to
/// <https://html.spec.whatwg.org/multipage/#rules-for-parsing-integers>.
pub fn parse_integer<T: Iterator<Item=char>>(input: T) -> Option<i32> {
    do_parse_integer(input).and_then(|result| {
        result.to_i32()
    })
}

/// Parse an integer according to
/// <https://html.spec.whatwg.org/multipage/#rules-for-parsing-non-negative-integers>
pub fn parse_unsigned_integer<T: Iterator<Item=char>>(input: T) -> Option<u32> {
    do_parse_integer(input).and_then(|result| {
        result.to_u32()
    })
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum LengthOrPercentageOrAuto {
    Auto,
    Percentage(f32),
    Length(Au),
}

/// TODO: this function can be rewritten to return Result<LengthOrPercentage, _>
/// Parses a dimension value per HTML5 § 2.4.4.4. If unparseable, `Auto` is
/// returned.
/// https://html.spec.whatwg.org/multipage/#rules-for-parsing-dimension-values
pub fn parse_length(mut value: &str) -> LengthOrPercentageOrAuto {
    // Steps 1 & 2 are not relevant

    // Step 3
    value = value.trim_left_matches(WHITESPACE);

    // Step 4
    if value.is_empty() {
        return LengthOrPercentageOrAuto::Auto
    }

    // Step 5
    if value.starts_with("+") {
        value = &value[1..]
    }

    // Steps 6 & 7
    match value.chars().nth(0) {
        Some('0'...'9') => {},
        _ => return LengthOrPercentageOrAuto::Auto,
    }

    // Steps 8 to 13
    // We trim the string length to the minimum of:
    // 1. the end of the string
    // 2. the first occurence of a '%' (U+0025 PERCENT SIGN)
    // 3. the second occurrence of a '.' (U+002E FULL STOP)
    // 4. the occurrence of a character that is neither a digit nor '%' nor '.'
    // Note: Step 10 is directly subsumed by FromStr::from_str
    let mut end_index = value.len();
    let (mut found_full_stop, mut found_percent) = (false, false);
    for (i, ch) in value.chars().enumerate() {
        match ch {
            '0'...'9' => continue,
            '%' => {
                found_percent = true;
                end_index = i;
                break
            }
            '.' if !found_full_stop => {
                found_full_stop = true;
                continue
            }
            _ => {
                end_index = i;
                break
            }
        }
    }
    value = &value[..end_index];

    if found_percent {
        let result: Result<f32, _> = FromStr::from_str(value);
        match result {
            Ok(number) => return LengthOrPercentageOrAuto::Percentage((number as f32) / 100.0),
            Err(_) => return LengthOrPercentageOrAuto::Auto,
        }
    }

    match FromStr::from_str(value) {
        Ok(number) => LengthOrPercentageOrAuto::Length(Au::from_f64_px(number)),
        Err(_) => LengthOrPercentageOrAuto::Auto,
    }
}

/// https://html.spec.whatwg.org/multipage/#rules-for-parsing-a-legacy-font-size
pub fn parse_legacy_font_size(mut input: &str) -> Option<&'static str> {
    // Steps 1 & 2 are not relevant

    // Step 3
    input = input.trim_matches(WHITESPACE);

    enum ParseMode {
        RelativePlus,
        RelativeMinus,
        Absolute,
    }
    let mut input_chars = input.chars().peekable();
    let parse_mode = match input_chars.peek() {
        // Step 4
        None => return None,

        // Step 5
        Some(&'+') => {
            let _ = input_chars.next();  // consume the '+'
            ParseMode::RelativePlus
        }
        Some(&'-') => {
            let _ = input_chars.next();  // consume the '-'
            ParseMode::RelativeMinus
        }
        Some(_) => ParseMode::Absolute,
    };

    // Steps 6, 7, 8
    let mut value = match read_numbers(input_chars) {
        Some(v) => v,
        None => return None,
    };

    // Step 9
    match parse_mode {
        ParseMode::RelativePlus => value = 3 + value,
        ParseMode::RelativeMinus => value = 3 - value,
        ParseMode::Absolute => (),
    }

    // Steps 10, 11, 12
    Some(match value {
        n if n >= 7 => "xxx-large",
        6 => "xx-large",
        5 => "x-large",
        4 => "large",
        3 => "medium",
        2 => "small",
        n if n <= 1 => "x-small",
        _ => unreachable!(),
    })
}

/// Parses a legacy color per HTML5 § 2.4.6. If unparseable, `Err` is returned.
pub fn parse_legacy_color(mut input: &str) -> Result<RGBA, ()> {
    // Steps 1 and 2.
    if input.is_empty() {
        return Err(())
    }

    // Step 3.
    input = input.trim_matches(WHITESPACE);

    // Step 4.
    if input.eq_ignore_ascii_case("transparent") {
        return Err(())
    }

    // Step 5.
    if let Ok(Color::RGBA(rgba)) = cssparser::parse_color_keyword(input) {
        return Ok(rgba);
    }

    // Step 6.
    if input.len() == 4 {
        match (input.as_bytes()[0],
               hex(input.as_bytes()[1] as char),
               hex(input.as_bytes()[2] as char),
               hex(input.as_bytes()[3] as char)) {
            (b'#', Ok(r), Ok(g), Ok(b)) => {
                return Ok(RGBA {
                    red: (r as f32) * 17.0 / 255.0,
                    green: (g as f32) * 17.0 / 255.0,
                    blue: (b as f32) * 17.0 / 255.0,
                    alpha: 1.0,
                })
            }
            _ => {}
        }
    }

    // Step 7.
    let mut new_input = String::new();
    for ch in input.chars() {
        if ch as u32 > 0xffff {
            new_input.push_str("00")
        } else {
            new_input.push(ch)
        }
    }
    let mut input = &*new_input;

    // Step 8.
    for (char_count, (index, _)) in input.char_indices().enumerate() {
        if char_count == 128 {
            input = &input[..index];
            break
        }
    }

    // Step 9.
    if input.as_bytes()[0] == b'#' {
        input = &input[1..]
    }

    // Step 10.
    let mut new_input = Vec::new();
    for ch in input.chars() {
        if hex(ch).is_ok() {
            new_input.push(ch as u8)
        } else {
            new_input.push(b'0')
        }
    }
    let mut input = new_input;

    // Step 11.
    while input.is_empty() || (input.len() % 3) != 0 {
        input.push(b'0')
    }

    // Step 12.
    let mut length = input.len() / 3;
    let (mut red, mut green, mut blue) = (&input[..length],
                                          &input[length..length * 2],
                                          &input[length * 2..]);

    // Step 13.
    if length > 8 {
        red = &red[length - 8..];
        green = &green[length - 8..];
        blue = &blue[length - 8..];
        length = 8
    }

    // Step 14.
    while length > 2 && red[0] == b'0' && green[0] == b'0' && blue[0] == b'0' {
        red = &red[1..];
        green = &green[1..];
        blue = &blue[1..];
        length -= 1
    }

    // Steps 15-20.
    return Ok(RGBA {
        red: hex_string(red).unwrap() as f32 / 255.0,
        green: hex_string(green).unwrap() as f32 / 255.0,
        blue: hex_string(blue).unwrap() as f32 / 255.0,
        alpha: 1.0,
    });

    fn hex(ch: char) -> Result<u8, ()> {
        match ch {
            '0'...'9' => Ok((ch as u8) - b'0'),
            'a'...'f' => Ok((ch as u8) - b'a' + 10),
            'A'...'F' => Ok((ch as u8) - b'A' + 10),
            _ => Err(()),
        }
    }

    fn hex_string(string: &[u8]) -> Result<u8, ()> {
        match string.len() {
            0 => Err(()),
            1 => hex(string[0] as char),
            _ => {
                let upper = try!(hex(string[0] as char));
                let lower = try!(hex(string[1] as char));
                Ok((upper << 4) | lower)
            }
        }
    }
}


#[derive(Clone, Eq, PartialEq, Hash, Debug, Deserialize, Serialize)]
pub struct LowercaseString {
    inner: String,
}

impl LowercaseString {
    pub fn new(s: &str) -> LowercaseString {
        LowercaseString {
            inner: s.to_lowercase(),
        }
    }
}

impl Deref for LowercaseString {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        &*self.inner
    }
}

/// Creates a String from the given null-terminated buffer.
/// Panics if the buffer does not contain UTF-8.
pub unsafe fn c_str_to_string(s: *const c_char) -> String {
    from_utf8(CStr::from_ptr(s).to_bytes()).unwrap().to_owned()
}

pub fn str_join<I, T>(strs: I, join: &str) -> String
    where I: IntoIterator<Item=T>, T: AsRef<str>,
{
    strs.into_iter().enumerate().fold(String::new(), |mut acc, (i, s)| {
        if i > 0 { acc.push_str(join); }
        acc.push_str(s.as_ref());
        acc
    })
}

// Lifted from Rust's StrExt implementation, which is being removed.
pub fn slice_chars(s: &str, begin: usize, end: usize) -> &str {
    assert!(begin <= end);
    let mut count = 0;
    let mut begin_byte = None;
    let mut end_byte = None;

    // This could be even more efficient by not decoding,
    // only finding the char boundaries
    for (idx, _) in s.char_indices() {
        if count == begin { begin_byte = Some(idx); }
        if count == end { end_byte = Some(idx); break; }
        count += 1;
    }
    if begin_byte.is_none() && count == begin { begin_byte = Some(s.len()) }
    if end_byte.is_none() && count == end { end_byte = Some(s.len()) }

    match (begin_byte, end_byte) {
        (None, _) => panic!("slice_chars: `begin` is beyond end of string"),
        (_, None) => panic!("slice_chars: `end` is beyond end of string"),
        (Some(a), Some(b)) => unsafe { s.slice_unchecked(a, b) }
    }
}

// searches a character index in CharIndices
// returns indices.count if not found
pub fn search_index(index: usize, indices: CharIndices) -> isize {
    let mut character_count = 0;
    for (character_index, _) in indices {
        if character_index == index {
            return character_count;
        }
        character_count += 1
    }
    character_count
}
