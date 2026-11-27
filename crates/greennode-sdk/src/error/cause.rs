use super::Error;

#[derive(Default)]
pub(super) struct CauseChain(pub(super) Option<Box<Error>>);

impl From<Error> for CauseChain {
    fn from(cause: Error) -> Self {
        Self(Some(Box::new(cause)))
    }
}

impl Drop for CauseChain {
    fn drop(&mut self) {
        let mut layer = self.0.take();
        while let Some(mut error) = layer {
            layer = error.cause.0.take();
        }
    }
}

impl Error {
    pub(super) fn chain(&self) -> impl Iterator<Item = &Self> {
        std::iter::successors(Some(self), |layer| layer.cause.0.as_deref())
    }

    pub(crate) fn screen_api(mut self, reflects: &impl Fn(&str) -> bool) -> Self {
        let mut layers = Vec::new();
        let mut cause = self.cause.0.take();
        while let Some(mut error) = cause {
            cause = error.cause.0.take();
            layers.push(error);
        }
        let mut screened = None;
        for mut layer in layers.into_iter().rev() {
            layer.cause.0 = screened;
            layer.screen_layer(reflects);
            screened = Some(layer);
        }
        self.cause.0 = screened;
        self.screen_layer(reflects);
        self
    }
}
