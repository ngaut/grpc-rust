use std::io;
use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;

use futures::done;
use futures::Future;
use futures::stream::Stream;

use tokio_core::io::read_exact;
use tokio_core::io::write_all;
use tokio_core::net::TcpStream;
use tokio_core::reactor;

use solicit::http::HttpError;
use solicit::http::frame::RawFrame;
use solicit::http::frame::FrameIR;
use solicit::http::frame::unpack_header;
use solicit::http::frame::settings::SettingsFrame;
use solicit::http::frame::settings::HttpSetting;
use solicit::http::connection::HttpFrame;


pub type HttpFuture<T> = Box<Future<Item=T, Error=HttpError>>;
pub type HttpStream<T> = Box<Stream<Item=T, Error=HttpError>>;

pub type HttpFutureSend<T> = Box<Future<Item=T, Error=HttpError> + Send>;
pub type HttpStreamSend<T> = Box<Stream<Item=T, Error=HttpError> + Send>;


struct VecWithPos<T> {
    vec: Vec<T>,
    pos: usize,
}

impl<T> AsMut<[T]> for VecWithPos<T> {
    fn as_mut(&mut self) -> &mut [T] {
        &mut self.vec[self.pos..]
    }
}

pub fn recv_raw_frame<R : Read + Send + 'static>(read: R) -> HttpFuture<(R, RawFrame<'static>)> {
    let header = read_exact(read, [0; 9]);
    let frame_buf = header.and_then(|(read, raw_header)| {
        let header = unpack_header(&raw_header);
        let total_len = 9 + header.0 as usize;
        let mut full_frame = VecWithPos {
            vec: Vec::with_capacity(9),
            pos: 0,
        };

        full_frame.vec.reserve_exact(total_len);
        full_frame.vec.extend(&raw_header);
        full_frame.vec.resize(total_len, 0);
        full_frame.pos = 9;

        println!("about to read_exact {}", full_frame.as_mut().len());
        read_exact(read, full_frame)
    });
    let frame = frame_buf.map(|(read, frame_buf)| {
        println!("after read_exact");
        (read, RawFrame::from(frame_buf.vec))
    });
    Box::new(frame
        .map_err(|e| e.into()))
}

#[allow(dead_code)]
pub fn recv_raw_frame_stream<R : Read + Send + 'static>(_read: R) -> HttpStream<RawFrame<'static>> {
    // https://users.rust-lang.org/t/futures-rs-how-to-generate-a-stream-from-futures/7020
    panic!();
}

pub fn recv_settings_frame<R : Read + Send + 'static>(read: R) -> HttpFuture<(R, SettingsFrame)> {
    Box::new(recv_raw_frame(read)
        .then(|result| {
            result.and_then(|(read, raw_frame)| {
                match HttpFrame::from_raw(&raw_frame) {
                    Ok(HttpFrame::SettingsFrame(f)) => Ok((read, f)),
                    Ok(_) => Err(HttpError::InvalidFrame),
                    Err(e) => Err(e),
                }
            })
        }))
}

#[allow(dead_code)]
pub fn send_raw_frame<W : Write + Send + 'static>(write: W, frame: RawFrame<'static>) -> HttpFuture<W> {
    let bytes = frame.serialize();
    Box::new(write_all(write, bytes)
        .map(|(w, _)| w)
        .map_err(|e| e.into()))
}

pub fn send_frame<W : Write + Send + 'static, F : FrameIR>(write: W, frame: F) -> HttpFuture<W> {
    let mut buf = io::Cursor::new(Vec::with_capacity(16));
    frame.serialize_into(&mut buf).unwrap();
    Box::new(write_all(write, buf.into_inner())
        .map(|(w, _)| w)
        .map_err(|e| e.into()))
}

static PREFACE: &'static [u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

pub fn client_handshake(conn: TcpStream) -> HttpFuture<TcpStream> {
    let send_preface = write_all(conn, PREFACE)
        .map(|(conn, _)| conn)
        .map_err(|e| e.into());

    let send_settings = send_preface.and_then(|conn| {
        let settings = {
            let mut frame = SettingsFrame::new();
            frame.add_setting(HttpSetting::EnablePush(0));
            frame
        };
        send_frame(conn, settings)
    });

    let recv_settings = send_settings.and_then(|conn| {
        recv_settings_frame(conn).map(|(conn, _)| conn)
    });

    let done = recv_settings;

    Box::new(done)
}

pub fn server_handshake(conn: TcpStream) -> HttpFuture<TcpStream> {
    let mut preface_buf = Vec::with_capacity(PREFACE.len());
    preface_buf.resize(PREFACE.len(), 0);
    let recv_preface = read_exact(conn, preface_buf)
        .map_err(|e| e.into())
        .and_then(|(conn, preface_buf)| {
            done(if preface_buf == PREFACE {
                Ok((conn))
            } else {
                Err(HttpError::InvalidFrame)
            })
        });

    let recv_settings = recv_preface.and_then(|conn| {
        recv_settings_frame(conn).map(|(conn, _)| conn)
    });

    let send_settings = recv_settings.and_then(|conn| {
        send_frame(conn, SettingsFrame::new_ack())
    });

    Box::new(send_settings)
}

pub fn connect_and_handshake(lh: &reactor::Handle, addr: &SocketAddr) -> HttpFuture<TcpStream> {
    let connect = TcpStream::connect(&addr, lh)
        .map_err(|e| e.into());

    let handshake = connect.and_then(client_handshake);

    Box::new(handshake)
}
