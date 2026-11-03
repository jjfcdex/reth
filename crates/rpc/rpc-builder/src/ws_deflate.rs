//! Decode-only `permessage-deflate` (RFC 7692) extension for the WS RPC server.
//!
//! Clients may compress the messages they send; the server inflates them but never compresses
//! its own messages.
//!
//! Every message is inflated on its own (no context takeover). The negotiation response therefore
//! always carries `server_no_context_takeover; client_no_context_takeover`: RFC 7692 §7.1.1 lets
//! a server add both even if the client did not offer them, and the client must comply. This is
//! the same response gorilla/websocket (geth family) sends, and gorilla clients reject a response
//! that lacks either parameter.
//!
//! The inflated size of a message is capped, otherwise a small compressed frame could expand to
//! gigabytes (soketto only limits the compressed size on the wire).

use flate2::{Decompress, FlushDecompress, Status};
use soketto::{
    base::{Header, OpCode},
    extension::{Extension, Param},
    BoxedError, Storage,
};
use tracing::debug;

const EXTENSION_NAME: &str = "permessage-deflate";
const SERVER_NO_CONTEXT_TAKEOVER: &str = "server_no_context_takeover";
const CLIENT_NO_CONTEXT_TAKEOVER: &str = "client_no_context_takeover";
const SERVER_MAX_WINDOW_BITS: &str = "server_max_window_bits";
const CLIENT_MAX_WINDOW_BITS: &str = "client_max_window_bits";

/// Removed by the sender from the end of every compressed message (RFC 7692 §7.2.1).
const DEFLATE_TRAILER: [u8; 4] = [0x00, 0x00, 0xff, 0xff];

/// Size of the scratch buffer used while inflating.
const INFLATE_CHUNK: usize = 16 * 1024;

#[derive(Debug, thiserror::Error)]
enum DeflateError {
    #[error("inflated WebSocket message exceeds the maximum of {max} bytes")]
    MessageTooLarge { max: usize },
    #[error("truncated or corrupt deflate data in WebSocket message")]
    Corrupt,
}

/// Server side `permessage-deflate` that only inflates client→server messages.
#[derive(Debug)]
pub(crate) struct DecodeOnlyDeflate {
    enabled: bool,
    params: Vec<Param<'static>>,
    max_message_size: usize,
    await_last_fragment: bool,
    /// Created once the extension is negotiated, so connections without compression don't pay
    /// for the inflate state.
    inflater: Option<Decompress>,
    output: Vec<u8>,
}

impl DecodeOnlyDeflate {
    /// Creates the extension. `max_message_size` caps the inflated size of a single message and
    /// should match the server's maximum request body size.
    pub(crate) const fn new(max_message_size: usize) -> Self {
        Self {
            enabled: false,
            params: Vec::new(),
            max_message_size,
            await_last_fragment: false,
            inflater: None,
            output: Vec::new(),
        }
    }

    /// Validates one offer and returns the response parameters, or `None` to decline it.
    fn accept_offer(params: &[Param<'_>]) -> Option<Vec<Param<'static>>> {
        let mut seen = [false; 4];
        let mut server_max_window_bits = None;
        for p in params {
            let idx = match p.name() {
                SERVER_NO_CONTEXT_TAKEOVER => 0,
                CLIENT_NO_CONTEXT_TAKEOVER => 1,
                SERVER_MAX_WINDOW_BITS => 2,
                CLIENT_MAX_WINDOW_BITS => 3,
                other => {
                    debug!(target: "rpc::ws", param = other, "declining permessage-deflate offer: unknown parameter");
                    return None
                }
            };
            if std::mem::replace(&mut seen[idx], true) {
                debug!(target: "rpc::ws", param = p.name(), "declining permessage-deflate offer: duplicate parameter");
                return None
            }
            match idx {
                0 | 1 if p.value().is_some() => return None,
                // We never compress, so any limit on our window is trivially met; echo it back.
                2 => server_max_window_bits = Some(window_bits(p.value()?)?),
                // A 15 bit inflate window decodes data produced with any smaller window, so the
                // client's value needs no answer.
                3 => {
                    if let Some(v) = p.value() {
                        window_bits(v)?;
                    }
                }
                _ => {}
            }
        }

        let mut response =
            vec![Param::new(SERVER_NO_CONTEXT_TAKEOVER), Param::new(CLIENT_NO_CONTEXT_TAKEOVER)];
        if let Some(bits) = server_max_window_bits {
            let mut p = Param::new(SERVER_MAX_WINDOW_BITS);
            p.set_value(Some(bits.to_string()));
            response.push(p);
        }
        Some(response)
    }

    /// Inflates one complete message from `data` into `self.output`.
    fn inflate(&mut self, data: &mut Vec<u8>) -> Result<(), DeflateError> {
        data.extend_from_slice(&DEFLATE_TRAILER);
        let inflater = self.inflater.get_or_insert_with(|| Decompress::new(false));
        // No context takeover: every message starts from an empty window.
        inflater.reset(false);
        self.output.clear();

        let mut chunk = [0u8; INFLATE_CHUNK];
        loop {
            let consumed = inflater.total_in() as usize;
            let out_before = inflater.total_out();
            let status = inflater
                .decompress(&data[consumed..], &mut chunk, FlushDecompress::Sync)
                .map_err(|_| DeflateError::Corrupt)?;
            let produced = (inflater.total_out() - out_before) as usize;
            if self.output.len() + produced > self.max_message_size {
                return Err(DeflateError::MessageTooLarge { max: self.max_message_size })
            }
            self.output.extend_from_slice(&chunk[..produced]);

            let all_consumed = inflater.total_in() as usize == data.len();
            if status == Status::StreamEnd || (all_consumed && produced < chunk.len()) {
                return Ok(())
            }
            if produced == 0 && inflater.total_in() as usize == consumed {
                return Err(DeflateError::Corrupt)
            }
        }
    }
}

fn window_bits(value: &str) -> Option<u8> {
    value.parse::<u8>().ok().filter(|bits| (8..=15).contains(bits))
}

impl Extension for DecodeOnlyDeflate {
    fn name(&self) -> &str {
        EXTENSION_NAME
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn params(&self) -> &[Param<'_>] {
        &self.params
    }

    fn configure(&mut self, params: &[Param<'_>]) -> Result<(), BoxedError> {
        // soketto calls this once per offer, in the client's order of preference. Keep the first
        // acceptable one.
        if self.enabled {
            return Ok(())
        }
        if let Some(response) = Self::accept_offer(params) {
            self.params = response;
            self.enabled = true;
        }
        Ok(())
    }

    fn reserved_bits(&self) -> (bool, bool, bool) {
        (true, false, false)
    }

    fn decode(&mut self, header: &mut Header, data: &mut Vec<u8>) -> Result<(), BoxedError> {
        // soketto passes the whole message: for a fragmented message the first fragment is
        // skipped here and the accumulated payload arrives with the final CONTINUE frame.
        match header.opcode() {
            OpCode::Text | OpCode::Binary if header.is_rsv1() => {
                if !header.is_fin() {
                    self.await_last_fragment = true;
                    return Ok(())
                }
            }
            OpCode::Continue if header.is_fin() && self.await_last_fragment => {
                self.await_last_fragment = false;
            }
            _ => return Ok(()),
        }

        if !data.is_empty() {
            if let Err(err) = self.inflate(data) {
                debug!(target: "rpc::ws", %err, "failed to inflate WebSocket message");
                return Err(Box::new(err))
            }
            std::mem::swap(data, &mut self.output);
        }
        header.set_rsv1(false);
        header.set_payload_len(data.len());
        Ok(())
    }

    fn encode(&mut self, _header: &mut Header, _data: &mut Storage<'_>) -> Result<(), BoxedError> {
        // Responses are sent uncompressed (RSV1 stays unset).
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compress, Compression, FlushCompress};

    const LIMIT: usize = 15 * 1024 * 1024;
    const M0: &str = r#"{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":0}"#;
    const M1: &str = r#"{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}"#;

    fn offer(params: &[(&'static str, Option<&'static str>)]) -> Vec<Param<'static>> {
        params
            .iter()
            .map(|(name, value)| {
                let mut p = Param::new(*name);
                p.set_value(*value);
                p
            })
            .collect()
    }

    fn negotiate(offers: &[Vec<Param<'static>>]) -> DecodeOnlyDeflate {
        let mut ext = DecodeOnlyDeflate::new(LIMIT);
        for o in offers {
            ext.configure(o).unwrap();
        }
        ext
    }

    /// Response parameters as soketto writes them into `Sec-WebSocket-Extensions`.
    fn response(ext: &DecodeOnlyDeflate) -> Vec<String> {
        ext.params()
            .iter()
            .map(|p| match p.value() {
                Some(v) => format!("{}={v}", p.name()),
                None => p.name().to_string(),
            })
            .collect()
    }

    /// Compresses each message with `compressor`, ending with a sync flush and stripping the
    /// trailer, as a permessage-deflate sender does.
    fn compress_with(compressor: &mut Compress, msg: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(msg.len() / 2 + 64);
        loop {
            let consumed = compressor.total_in();
            compressor
                .compress_vec(&msg[consumed as usize..], &mut out, FlushCompress::None)
                .unwrap();
            if compressor.total_in() as usize == msg.len() {
                break
            }
            out.reserve(64 * 1024);
        }
        loop {
            out.reserve(64 * 1024);
            compressor.compress_vec(&[], &mut out, FlushCompress::Sync).unwrap();
            if out.ends_with(&DEFLATE_TRAILER) {
                break
            }
        }
        out.truncate(out.len() - DEFLATE_TRAILER.len());
        out
    }

    fn compress(msg: &[u8]) -> Vec<u8> {
        compress_with(&mut Compress::new(Compression::default(), false), msg)
    }

    fn frame(opcode: OpCode, fin: bool, rsv1: bool, len: usize) -> Header {
        let mut h = Header::new(opcode);
        h.set_fin(fin);
        h.set_rsv1(rsv1);
        h.set_payload_len(len);
        h
    }

    fn decode_message(
        ext: &mut DecodeOnlyDeflate,
        compressed: Vec<u8>,
    ) -> Result<Vec<u8>, BoxedError> {
        let mut data = compressed;
        let mut h = frame(OpCode::Text, true, true, data.len());
        ext.decode(&mut h, &mut data)?;
        assert!(!h.is_rsv1());
        assert_eq!(h.payload_len(), data.len());
        Ok(data)
    }

    #[test]
    fn response_always_forces_no_context_takeover() {
        let expected = vec![SERVER_NO_CONTEXT_TAKEOVER, CLIENT_NO_CONTEXT_TAKEOVER];
        // wscat / Node ws, browsers, a bare offer, Java-WebSocket and gorilla.
        for o in [
            offer(&[(CLIENT_MAX_WINDOW_BITS, None)]),
            offer(&[]),
            offer(&[(SERVER_NO_CONTEXT_TAKEOVER, None), (CLIENT_NO_CONTEXT_TAKEOVER, None)]),
            offer(&[(CLIENT_NO_CONTEXT_TAKEOVER, None), (CLIENT_MAX_WINDOW_BITS, Some("12"))]),
        ] {
            let ext = negotiate(&[o]);
            assert!(ext.is_enabled());
            assert_eq!(response(&ext), expected);
        }
    }

    #[test]
    fn echoes_server_max_window_bits() {
        let ext = negotiate(&[offer(&[(SERVER_MAX_WINDOW_BITS, Some("10"))])]);
        assert!(ext.is_enabled());
        assert_eq!(
            response(&ext),
            vec![
                SERVER_NO_CONTEXT_TAKEOVER,
                CLIENT_NO_CONTEXT_TAKEOVER,
                "server_max_window_bits=10"
            ]
        );
    }

    #[test]
    fn declines_invalid_offers() {
        for o in [
            offer(&[("x_unknown", None)]),
            offer(&[(SERVER_NO_CONTEXT_TAKEOVER, None), (SERVER_NO_CONTEXT_TAKEOVER, None)]),
            offer(&[(CLIENT_NO_CONTEXT_TAKEOVER, Some("1"))]),
            offer(&[(SERVER_MAX_WINDOW_BITS, None)]),
            offer(&[(SERVER_MAX_WINDOW_BITS, Some("16"))]),
            offer(&[(CLIENT_MAX_WINDOW_BITS, Some("7"))]),
        ] {
            let ext = negotiate(&[o]);
            assert!(!ext.is_enabled());
            assert!(ext.params().is_empty());
        }
    }

    #[test]
    fn first_acceptable_offer_wins() {
        let ext = negotiate(&[
            offer(&[("x_unknown", None)]),
            offer(&[(SERVER_MAX_WINDOW_BITS, Some("9"))]),
            offer(&[]),
        ]);
        assert!(ext.is_enabled());
        assert_eq!(response(&ext).last().unwrap(), "server_max_window_bits=9");
    }

    #[test]
    fn decodes_independently_compressed_messages() {
        let mut ext = negotiate(&[offer(&[])]);
        for m in [M0, M1, M0, M1] {
            assert_eq!(decode_message(&mut ext, compress(m.as_bytes())).unwrap(), m.as_bytes());
        }
    }

    #[test]
    fn decodes_message_ending_with_final_block() {
        let mut ext = negotiate(&[offer(&[])]);
        let mut out = Vec::with_capacity(256);
        Compress::new(Compression::default(), false)
            .compress_vec(M0.as_bytes(), &mut out, FlushCompress::Finish)
            .unwrap();
        assert_eq!(decode_message(&mut ext, out).unwrap(), M0.as_bytes());
        assert_eq!(decode_message(&mut ext, compress(M1.as_bytes())).unwrap(), M1.as_bytes());
    }

    #[test]
    fn decodes_large_and_fragmented_messages() {
        let mut ext = negotiate(&[offer(&[])]);
        let big: Vec<u8> = (0..3_000_000u32).flat_map(|i| (i % 251).to_le_bytes()).collect();
        let compressed = compress(&big);
        assert_eq!(decode_message(&mut ext, compressed.clone()).unwrap(), big);

        // First fragment carries RSV1, the final CONTINUE frame brings the whole payload.
        let split = compressed.len() / 2;
        let mut data = compressed[..split].to_vec();
        let mut first = frame(OpCode::Binary, false, true, data.len());
        ext.decode(&mut first, &mut data).unwrap();
        assert_eq!(data, compressed[..split]);
        data.extend_from_slice(&compressed[split..]);
        let mut last = frame(OpCode::Continue, true, false, data.len());
        ext.decode(&mut last, &mut data).unwrap();
        assert_eq!(data, big);
    }

    #[test]
    fn passes_uncompressed_and_empty_messages_through() {
        let mut ext = negotiate(&[offer(&[])]);
        let mut data = M0.as_bytes().to_vec();
        let mut h = frame(OpCode::Text, true, false, data.len());
        ext.decode(&mut h, &mut data).unwrap();
        assert_eq!(data, M0.as_bytes());

        assert!(decode_message(&mut ext, Vec::new()).unwrap().is_empty());
    }

    #[test]
    fn rejects_message_inflating_past_limit() {
        let mut ext = DecodeOnlyDeflate::new(1024 * 1024);
        ext.configure(&offer(&[])).unwrap();
        let at_limit = vec![b' '; 1024 * 1024];
        assert_eq!(decode_message(&mut ext, compress(&at_limit)).unwrap().len(), at_limit.len());

        let bomb = compress(&vec![b' '; 64 * 1024 * 1024]);
        assert!(bomb.len() < 128 * 1024, "compressed bomb is {} bytes", bomb.len());
        let err = decode_message(&mut ext, bomb).unwrap_err();
        assert!(err.to_string().contains("exceeds the maximum"), "{err}");

        // The connection-level state is still usable for the next message.
        assert_eq!(decode_message(&mut ext, compress(M0.as_bytes())).unwrap(), M0.as_bytes());
    }

    #[test]
    fn rejects_corrupt_data() {
        let mut ext = negotiate(&[offer(&[])]);
        assert!(decode_message(&mut ext, vec![0xff; 32]).is_err());
    }

    #[test]
    fn context_takeover_data_is_not_decodable() {
        // Documents why the response must force client_no_context_takeover.
        let mut ext = negotiate(&[offer(&[])]);
        let mut shared = Compress::new(Compression::default(), false);
        let first = compress_with(&mut shared, M0.as_bytes());
        let second = compress_with(&mut shared, M1.as_bytes());
        assert_eq!(decode_message(&mut ext, first).unwrap(), M0.as_bytes());
        assert!(decode_message(&mut ext, second).map_or(true, |m| m != M1.as_bytes()));
    }
}
