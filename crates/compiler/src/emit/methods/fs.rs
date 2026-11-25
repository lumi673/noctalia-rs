use super::*;

impl<'a> Emitter<'a> {
    /// `std::fs` receivers: S3 adds the arms here.
    pub(super) fn fs_method(
        &mut self,
        _call: &ExprMethodCall,
        _method: &str,
        _args: &[&Expr],
        _receiver: &str,
        _ty: Ty,
    ) -> Result<OptionalValue, Error> {
        Ok(None)
    }
}
