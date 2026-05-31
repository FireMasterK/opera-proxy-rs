use tokio::io::{self, AsyncRead, AsyncWrite};

pub async fn tunnel_bidirectional<A, B>(left: &mut A, right: &mut B) -> io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    io::copy_bidirectional(left, right).await
}
