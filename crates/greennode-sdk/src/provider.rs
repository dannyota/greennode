use crate::{Error, RequestContext, Secret, Token};
use std::{future::Future, pin::Pin};

/// Supplies bearer credentials and handles rejected tokens.
pub trait CredentialsProvider: Send + Sync {
    /// Returns a token, cooperating with the request context's cancellation and deadline.
    fn token<'a>(
        &'a self,
        ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>>;

    /// Invalidates the rejected token if it still matches the provider's cached token.
    fn invalidate<'a>(
        &'a self,
        ctx: &'a RequestContext,
        rejected: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;
}
