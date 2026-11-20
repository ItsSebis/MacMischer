//! OSC encode/decode helpers, tolerant of X Air quirks (e.g. `/node` replies use address `node`).

use crate::param::{Param, Value};
use rosc::{decoder, encoder, OscMessage, OscPacket, OscType};

/// Encode a message. `args` empty = query.
pub fn encode(addr: &str, args: Vec<OscType>) -> Vec<u8> {
    encoder::encode(&OscPacket::Message(OscMessage { addr: addr.into(), args }))
        .expect("OSC encode of a plain message cannot fail")
}

pub fn encode_query(p: Param) -> Vec<u8> {
    encode(&p.path(), vec![])
}

pub fn encode_set(p: Param, v: &Value) -> Vec<u8> {
    encode(&p.path(), vec![to_osc(v)])
}

pub fn to_osc(v: &Value) -> OscType {
    match v {
        Value::Float(f) => OscType::Float(*f),
        Value::Int(i) => OscType::Int(*i),
        Value::Str(s) => OscType::String(s.clone()),
    }
}

pub fn from_osc(t: &OscType) -> Option<Value> {
    match t {
        OscType::Float(f) => Some(Value::Float(*f)),
        OscType::Int(i) => Some(Value::Int(*i)),
        OscType::String(s) => Some(Value::Str(s.clone())),
        _ => None,
    }
}

/// Decode a UDP datagram into messages (bundles flattened).
/// Fixes addresses lacking the leading `/` (X Air `/node` replies).
pub fn decode(buf: &[u8]) -> Vec<OscMessage> {
    let fixed;
    let data = if !buf.is_empty() && buf[0] != b'/' && buf[0] != b'#' {
        fixed = fix_address(buf);
        &fixed[..]
    } else {
        buf
    };
    let mut out = Vec::new();
    if let Ok((_, pkt)) = decoder::decode_udp(data) {
        flatten(pkt, &mut out);
    }
    out
}

fn flatten(p: OscPacket, out: &mut Vec<OscMessage>) {
    match p {
        OscPacket::Message(m) => out.push(m),
        OscPacket::Bundle(b) => b.content.into_iter().for_each(|p| flatten(p, out)),
    }
}

/// Re-encode the address string with a leading `/`, re-padding to 4 bytes.
fn fix_address(buf: &[u8]) -> Vec<u8> {
    let Some(nul) = buf.iter().position(|&b| b == 0) else {
        return buf.to_vec();
    };
    let old_len = (nul + 4) & !3; // padded length incl. at least one NUL
    let mut v = Vec::with_capacity(buf.len() + 4);
    v.push(b'/');
    v.extend_from_slice(&buf[..nul]);
    v.push(0);
    while v.len() % 4 != 0 {
        v.push(0);
    }
    v.extend_from_slice(buf.get(old_len..).unwrap_or(&[]));
    v
}

/// A decoded parameter update, if the message maps to a known [`Param`].
pub fn as_param_update(m: &OscMessage) -> Option<(Param, Value)> {
    let p = Param::parse(&m.addr)?;
    let v = from_osc(m.args.first()?)?;
    Some((p, v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strip::StripId;

    #[test]
    fn set_round_trip() {
        let p = Param::Fader(StripId::Ch(5));
        let msgs = decode(&encode_set(p, &Value::Float(0.5)));
        assert_eq!(as_param_update(&msgs[0]), Some((p, Value::Float(0.5))));
    }

    #[test]
    fn node_reply_without_slash() {
        // "node\0\0\0\0" ",s\0\0" "abc\0"
        let mut raw = b"node\0\0\0\0,s\0\0".to_vec();
        raw.extend_from_slice(b"abc\0");
        let msgs = decode(&raw);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].addr, "/node");
        assert_eq!(msgs[0].args, vec![OscType::String("abc".into())]);
    }
}
