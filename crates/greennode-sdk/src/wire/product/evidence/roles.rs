use super::{SentCredentials, Source};

impl SentCredentials {
    pub(in crate::wire::product) fn transport_only(&self) -> Self {
        if self
            .sources
            .iter()
            .any(|source| matches!(source, Source::Issued | Source::Redaction))
        {
            self.filtered(|_, source| !matches!(source, Source::Issued | Source::Redaction))
        } else {
            self.clone()
        }
    }

    pub(in crate::wire::product) fn wire_credentials(&self) -> Self {
        if self
            .sources
            .iter()
            .all(|role| matches!(role, Source::Wire | Source::Credential | Source::Bearer))
        {
            self.clone()
        } else {
            self.filtered(|_, role| {
                matches!(role, Source::Wire | Source::Credential | Source::Bearer)
            })
        }
    }

    pub(in crate::wire::product) fn guard_credentials(&self) -> Self {
        if self.sources.contains(&Source::Redaction) {
            self.filtered(|_, source| source != Source::Redaction)
        } else {
            self.clone()
        }
    }
}

impl super::Evidence {
    pub(crate) fn keep_acknowledgement(
        &mut self,
        id: &super::super::guard::AcknowledgedId,
    ) -> Result<(), Box<crate::Error>> {
        id.recheck(self)?;
        self.acknowledged = Some(id.clone());
        Ok(())
    }

    pub(in crate::wire::product) fn check_acknowledged(
        &self,
        credentials: &SentCredentials,
    ) -> Option<Box<crate::Error>> {
        self.acknowledged.as_ref()?.refusal(credentials)
    }
}
