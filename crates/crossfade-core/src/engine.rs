use crate::{AudioFrame, Error, Packet, StreamInfo};

/// Reads packets out of a container.
pub trait Demuxer {
    fn streams(&self) -> &[StreamInfo];

    /// Clears `out.data` and refills it with the next packet. `Ok(false)`
    /// means end of stream; `out` is then unspecified.
    fn read_packet(&mut self, out: &mut Packet) -> Result<bool, Error>;
}

/// Writes packets into a container.
pub trait Muxer {
    fn write_packet(&mut self, packet: &Packet) -> Result<(), Error>;

    /// Writes trailers and flushes. Call exactly once, after the last packet.
    fn finish(&mut self) -> Result<(), Error>;
}

/// Packets in, frames out. Lossy codecs buffer, so this is not one-in-one-out:
/// call `receive_frame` until it returns `Ok(false)` before the next
/// `send_packet`.
pub trait Decoder {
    fn send_packet(&mut self, packet: &Packet) -> Result<(), Error>;

    /// Clears and refills `out`. `Ok(false)`: nothing more until more input
    /// (or, after `drain`, nothing at all).
    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error>;

    /// Signals end of input; `receive_frame` then returns what is left and
    /// finally `Ok(false)`.
    fn drain(&mut self);
}

/// Frames in, packets out; same protocol as [`Decoder`]: call
/// `receive_packet` until it returns `Ok(false)` before the next `send_frame`.
pub trait Encoder {
    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error>;

    /// Clears and refills `out`. `Ok(false)`: nothing more until more input
    /// (or, after `drain`, nothing at all).
    fn receive_packet(&mut self, out: &mut Packet) -> Result<bool, Error>;

    /// Signals end of input; `receive_packet` then returns what is left and
    /// finally `Ok(false)`.
    fn drain(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Bare;

    impl Demuxer for Bare {
        fn streams(&self) -> &[StreamInfo] {
            &[]
        }
        fn read_packet(&mut self, _: &mut Packet) -> Result<bool, Error> {
            Ok(false)
        }
    }

    impl Muxer for Bare {
        fn write_packet(&mut self, _: &Packet) -> Result<(), Error> {
            Ok(())
        }
        fn finish(&mut self) -> Result<(), Error> {
            Ok(())
        }
    }

    impl Encoder for Bare {
        fn send_frame(&mut self, _: &AudioFrame) -> Result<(), Error> {
            Ok(())
        }
        fn receive_packet(&mut self, _: &mut Packet) -> Result<bool, Error> {
            Ok(false)
        }
        fn drain(&mut self) {}
    }

    #[test]
    fn the_new_seams_default_to_nothing() {
        assert!(Demuxer::tags(&Bare).is_empty());
        assert!(Encoder::extradata(&Bare).is_empty());
        assert_eq!(Muxer::set_extradata(&mut Bare, &[1, 2]), Ok(()));
    }
}
