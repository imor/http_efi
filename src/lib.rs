#![no_std]

extern crate efi;
extern crate httparse;

use core::cmp;

use efi::{
    EfiError,
    EfiErrorKind,
    net::{
        Tcp4Stream,
        SocketAddrV4,
    },
    string::ToString,
    io::{Read, Write}
};

use efi::Vec;

pub use httparse::Header;

pub type Result<T> = core::result::Result<T, EfiError>;

const MAX_HEADERS: usize = 30;

#[derive(Debug, Copy, Clone)]
pub struct StatusCode(u16);

pub struct Response<'a> {
    status_code: StatusCode,
    headers: Option<&'a [Header<'a>]>,
    body: &'a [u8],
}

impl<'a> Response<'a> {
    pub fn status_code(&self) -> StatusCode {
        self.status_code
    }

    pub fn headers(&self) -> Option<&'a [Header<'a>]> {
        self.headers
    }

    pub fn body(&self) -> &[u8] {
        self.body
    }
}

pub struct Client<'a> {
    io: BufWriter<Tcp4Stream>,
    headers: [Header<'a>; MAX_HEADERS],
    resp_buf: Vec<u8>,
}

impl<'a> Client<'a> {
    pub fn connect(addr: SocketAddrV4) -> Result<Self> {
        let tcp_stream = Tcp4Stream::connect(addr)?;
        Ok(Self { io: BufWriter::new(tcp_stream), headers: [httparse::EMPTY_HEADER; MAX_HEADERS], resp_buf: Vec::new() })
    }

    // TODO: This is wrong interface. Need to take Url here. Can't do that until we have a viable DNS client
    // TODO: Make it so that self is not mut in this call
    pub fn request<'b>(&'a mut self, method: &str, path: &str, headers: &[Header<'b>], body: Option<&[u8]>) -> Result<Response> {
        self.io.write(method.as_bytes())?;
        self.io.write(" ".as_bytes())?;
        self.io.write(path.as_bytes())?;
        self.io.write(" ".as_bytes())?;
        self.io.write("HTTP/1.1\r\n".as_bytes())?;

        fn write_hdr(io: &mut BufWriter<Tcp4Stream>, name: &str, value: &[u8]) -> Result<()> {
            io.write(name.as_bytes())?;
            io.write(":".as_bytes())?;
            io.write(value)?;
            io.write("\r\n".as_bytes())?;
            Ok(())
        }

        for header in headers {
            write_hdr(&mut self.io, header.name, header.value)?;
        }

        write_hdr(&mut self.io, "Content-Length", body.unwrap_or(&[]).len().to_string().as_bytes())?; // TODO: Don't do this if Content-Length is already present in incoming headers argument

        self.io.write("\r\n".as_bytes())?;

        // TODO: Should we add/honour Connection: (keep-alive|close) headers. Otherwise we may see dropped connections especially by proxies
        if let Some(body) = body {
            self.io.write("\r\n".as_bytes())?;
            self.io.write(body)?;
        }

        self.io.flush()?;

        // TODO: does the resposne on the wire always match the request we just sent?
        // Could there not be stray bytes from an earlier response?
        let total_len = self.io.read_to_end(&mut self.resp_buf)?;
        let mut head = httparse::Response::new(&mut self.headers);
        let parse_status = head.parse(&self.resp_buf).map_err(|_| { EfiError::from(EfiErrorKind::ProtocolError) })?;
        let parsed_len = match parse_status {
            httparse::Status::Complete(len) => len,
            httparse::Status::Partial => return Err(EfiError::from(EfiErrorKind::ProtocolError)),
        };

        let status_code = head.code.ok_or(EfiError::from(EfiErrorKind::ProtocolError))?;
        let body_start_pos = total_len - parsed_len;
        Ok(Response { 
            status_code: StatusCode(status_code),
            headers: if head.headers.len() != 0 { Some(head.headers) } else { None },
            body: &self.resp_buf[body_start_pos..] })
    }
}

const BUF_SIZE: usize = 1024; // TODO: Change size to something more optimal for TCP 

pub struct BufWriter<W: Write> {
        inner: W,
        buf: [u8; BUF_SIZE], // Fucking const generics still not implemented
        next_write_pos: usize
}

impl<W: Write> BufWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner, buf: [0; BUF_SIZE], next_write_pos: 0 }
    }

    fn flush(&mut self) -> Result<()> {
        let mut written = 0;
        while written < self.next_write_pos {
            match self.inner.write(&self.buf[written..self.next_write_pos]) {
                Ok(0) => {
                    return Err(EfiErrorKind::DeviceError.into())
                }
                Ok(n) => written += n,
                Err(e) => { return Err(e) }
            }
        }

        self.next_write_pos = 0;

        Ok(())
    }
}

impl<W: Write> Write for BufWriter<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> { // TODO: we need to create more failure types than just EfiError
        let mut next_read_pos = 0;
        let mut yet_to_write = buf.len();
        while yet_to_write > 0 {
            let mut available_space = self.buf.len() - self.next_write_pos;
            if available_space == 0 {
                self.flush()?;
                available_space = self.buf.len() - self.next_write_pos;
            }

            let write_size = cmp::min(yet_to_write, available_space);
            self.buf[self.next_write_pos..(self.next_write_pos + write_size)].copy_from_slice(&buf[next_read_pos..(next_read_pos + write_size)]);

            yet_to_write -= write_size;
            self.next_write_pos += write_size;
            next_read_pos += write_size;
        }

        Ok(buf.len())
    }
}

impl<W: Read + Write> Read for BufWriter<W> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.inner.read(buf)
    }
}