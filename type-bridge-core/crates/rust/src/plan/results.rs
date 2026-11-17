use super::*;
use type_bridge_orm::match_request::PlanFetchCell;

mod sealed {
    pub trait Cell {}
}

/// Sealed generated-model or canonical scalar cell of an authored tuple.
#[doc(hidden)]
pub trait PlanCellResult: sealed::Cell + Sized {
    fn decode_cell(
        cell: PlanFetchCell<'_>,
        projected: Option<(&ProjectedQueryValue, &InstalledRuntimeProjection)>,
    ) -> Result<Self>;
}

impl<M: crate::__codegen::MaterializeModel> sealed::Cell for M {}
impl<M: crate::__codegen::MaterializeModel> PlanCellResult for M {
    fn decode_cell(
        cell: PlanFetchCell<'_>,
        projected: Option<(&ProjectedQueryValue, &InstalledRuntimeProjection)>,
    ) -> Result<Self> {
        let PlanFetchCell::Model { row, slot } = cell else {
            return Err(result_shape());
        };
        let Some((ProjectedQueryValue::Rows { rows }, installed)) = projected else {
            return Err(result_shape());
        };
        let slot = rows
            .get(row)
            .and_then(|row| row.slots().get(slot))
            .ok_or_else(result_shape)?;
        let ProjectedQuerySlotValue::One(thing) = slot.value() else {
            return Err(result_shape());
        };
        let row = crate::projected_codec::projected_to_hydrated_row(thing, installed)?;
        M::materialize(&row, &crate::__codegen::HydrationCapability::new()).map_err(|error| {
            crate::entity_codec::map_validation_error(error, ModelValidationPhase::Hydration)
        })
    }
}

macro_rules! scalar_cell {
    ($ty:ty, $variant:ident, $value:ident, $convert:expr) => {
        impl sealed::Cell for $ty {}
        impl PlanCellResult for $ty {
            fn decode_cell(
                cell: PlanFetchCell<'_>,
                _projected: Option<(&ProjectedQueryValue, &InstalledRuntimeProjection)>,
            ) -> Result<Self> {
                let PlanFetchCell::Scalar(CanonicalValue::$variant($value)) = cell else {
                    return Err(result_shape());
                };
                $convert
            }
        }
    };
}
scalar_cell!(String, String, value, Ok(value.as_str().to_owned()));
scalar_cell!(i64, Long, value, Ok(*value));
scalar_cell!(bool, Boolean, value, Ok(*value));
scalar_cell!(f64, Double, value, Ok(value.get()));
scalar_cell!(
    CanonicalDouble,
    Double,
    value,
    CanonicalDouble::try_new(value.get()).map_err(|error| {
        crate::entity_codec::map_validation_error(error, ModelValidationPhase::Hydration)
    })
);
macro_rules! text_scalar_cell {
    ($ty:ty, $variant:ident) => {
        scalar_cell!(
            $ty,
            $variant,
            value,
            <$ty>::try_new(value.to_string()).map_err(|error| {
                crate::entity_codec::map_validation_error(error, ModelValidationPhase::Hydration)
            })
        );
    };
}
text_scalar_cell!(Date, Date);
text_scalar_cell!(DateTime, DateTime);
text_scalar_cell!(DateTimeTz, DateTimeTz);
text_scalar_cell!(Decimal, Decimal);
text_scalar_cell!(Duration, Duration);

macro_rules! tuple_result {
    ($($ty:ident:$index:tt),+) => {
        impl<$($ty: PlanCellResult),+> result_sealed::Sealed for Vec<($($ty,)+)> {}
        impl<$($ty: PlanCellResult),+> PlanResult for Vec<($($ty,)+)> {
            fn decode(plan: &QueryPlan, answer: &PlanAnswer) -> Result<Self> {
                answer.scalar_rows(plan).map_err(Error::from_contract_diagnostic)?.iter().map(|row| {
                    if row.len() != [$(stringify!($ty)),+].len() { return Err(result_shape()); }
                    Ok(($($ty::decode_cell(PlanFetchCell::Scalar(row.get($index).ok_or_else(result_shape)?), None)?,)+))
                }).collect()
            }
            fn decode_models(proof: &OwnedPlanFetchResult, projected: &ProjectedQueryValue, installed: &InstalledRuntimeProjection) -> Result<Self> {
                if proof.columns().len() != [$(stringify!($ty)),+].len() { return Err(result_shape()); }
                (0..proof.row_count()).map(|row| {
                    Ok(($($ty::decode_cell(proof.cell(row, *proof.columns().get($index).ok_or_else(result_shape)?).ok_or_else(result_shape)?, Some((projected, installed)))?,)+))
                }).collect()
            }
        }
    };
}
tuple_result!(A:0);
tuple_result!(A:0,B:1);
tuple_result!(A:0,B:1,C:2);
tuple_result!(A:0,B:1,C:2,D:3);
tuple_result!(A:0,B:1,C:2,D:3,E:4);
tuple_result!(A:0,B:1,C:2,D:3,E:4,F:5);
tuple_result!(A:0,B:1,C:2,D:3,E:4,F:5,G:6);
tuple_result!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7);
tuple_result!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8);
tuple_result!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9);
tuple_result!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10);
tuple_result!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11);
tuple_result!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11,M:12);
tuple_result!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11,M:12,N:13);
tuple_result!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11,M:12,N:13,O:14);
tuple_result!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11,M:12,N:13,O:14,P:15);
