use crate::{AudioFrame, AudioSpec, Error, NO_TAGS, Packet, StreamInfo, Tags};

/// Reads packets out of a container.
pub trait Demuxer {
    fn streams(&self) -> &[StreamInfo];

    /// Clears `out.data` and refills it with the next packet. `Ok(false)`
    /// means end of stream; `out` is then unspecified.
    fn read_packet(&mut self, out: &mut Packet) -> Result<bool, Error>;

    fn tags(&self) -> &Tags {
        &NO_TAGS
    }
}

/// Writes packets into a container.
pub trait Muxer {
    fn write_packet(&mut self, packet: &Packet) -> Result<(), Error>;

    /// The final codec header (see [`Encoder::extradata`]), given at most once, after the last
    /// packet and before `finish`.
    fn set_extradata(&mut self, _extradata: &[u8]) -> Result<(), Error> {
        Ok(())
    }

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

    /// Finishes decoding every packet sent so far without ending the stream, so that
    /// `receive_frame` can return the frames and a failure among them is reported (here or
    /// by `receive_frame`). A decoder that holds nothing back, the default, has nothing to do.
    /// A reader that fails calls this first, so an earlier frame's error is not hidden behind
    /// its own.
    fn flush(&mut self) -> Result<(), Error> {
        Ok(())
    }
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

    /// The codec header as known so far; final once `drain` has been called and every packet
    /// received.
    fn extradata(&self) -> &[u8] {
        &[]
    }
}

/// Frames in, frames out; same protocol as [`Decoder`]: call `receive_frame` until it returns
/// `Ok(false)` before the next `send_frame`, and after `drain` until it returns `Ok(false)`.
///
/// - Every frame sent matches the spec the filter was built for (rate, layout and sample
///   format); every frame received matches [`output_spec`](Self::output_spec), which is fixed
///   at construction. A filter may change rate, layout and length, and may hold frames back
///   (a look-ahead) until more input or `drain` arrives. It never returns an empty frame.
/// - Timing: output pts count output frames at the output rate, with no gaps. The first frame
///   out carries the first input pts rescaled to the output rate (rounded down), and each
///   later one the previous pts plus its frame count. So a trim's output starts where its
///   input started, and a resampler's output stays aligned with its input.
pub trait Filter {
    fn output_spec(&self) -> &AudioSpec;

    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error>;

    /// Clears and refills `out`. `Ok(false)`: nothing more until more input (or, after
    /// `drain`, nothing at all).
    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error>;

    /// Signals end of input; `receive_frame` then returns what is left and finally `Ok(false)`.
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
