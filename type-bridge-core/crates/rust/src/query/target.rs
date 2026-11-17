//! Target-selected terminal output and staged result decoding.

use super::*;

mod sealed {
    pub trait Sealed {}
}

/// Immediate typed query execution (the default session target).
#[derive(Debug)]
pub struct Direct;
/// Typed query execution queued on a staged transaction.
#[derive(Debug)]
pub struct Staged;
impl sealed::Sealed for Direct {}
impl sealed::Sealed for Staged {}

/// Sealed execution mode selecting each terminal's return type.
pub trait QueryTarget: sealed::Sealed + 'static {
    /// The immediate value or its staged pending result.
    type Out<T>;
    #[doc(hidden)]
    const STAGED: bool;
    #[doc(hidden)]
    fn immediate<T>(value: T) -> Self::Out<T>;
    #[doc(hidden)]
    fn pending<T>(value: Pending<T>) -> Self::Out<T>;
}
impl QueryTarget for Direct {
    type Out<T> = T;
    const STAGED: bool = false;
    fn immediate<T>(value: T) -> T {
        value
    }
    fn pending<T>(_value: Pending<T>) -> T {
        unreachable!("direct session cannot stage")
    }
}
impl QueryTarget for Staged {
    type Out<T> = Pending<T>;
    const STAGED: bool = true;
    fn immediate<T>(_value: T) -> Pending<T> {
        unreachable!("staged session cannot execute immediately")
    }
    fn pending<T>(value: Pending<T>) -> Pending<T> {
        value
    }
}

type PendingDecoder<T> = dyn Fn(&ValidatedMatchResult) -> Result<T>;

/// A typed query result, available only after a successful commit.
///
/// The pending owns its decoding context. Drop the authoring session before
/// committing its transaction; the pending remains usable afterwards.
pub struct Pending<T> {
    answer: type_bridge_orm::session::staged::Pending<ValidatedMatchResult>,
    decode: Box<PendingDecoder<T>>,
}
impl<T> std::fmt::Debug for Pending<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Pending")
            .field("answer", &self.answer)
            .finish_non_exhaustive()
    }
}
impl<T> Pending<T> {
    /// Materialize the terminal's exact output from its committed proof.
    ///
    /// # Errors
    /// Returns the pending state, transaction failure or hydration diagnostic.
    pub fn result(&self) -> Result<T> {
        (self.decode)(self.answer.get()?.as_ref())
    }
}

impl<'db, S: Schema> QuerySession<'db, S, Staged> {
    pub(crate) fn staged(
        installed: Arc<InstalledRuntimeProjection>,
        registry: Arc<DescriptorRegistry>,
        transaction: &'db mut type_bridge_orm::session::staged::Transaction,
        origin: ProjectedQueryOrigin,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Self {
        Self::new(
            installed,
            registry,
            QueryExecution::Staged {
                transaction: std::sync::Mutex::new(transaction),
                origin,
            },
            resources,
            cancellation,
        )
    }
}

impl<'s, 'db, S: Schema, Shape: SelectedShape<S>, Target: QueryTarget>
    Query<'s, 'db, S, Shape, Target>
{
    pub(super) async fn invoke<R: 'static>(
        &self,
        validated: ValidatedMatchRequest,
        deadline: QueryExecutionDeadline,
        decode: impl Fn(
            &Query<'_, '_, S, Shape, Target>,
            &ValidatedMatchRequest,
            &ValidatedMatchResult,
            QueryExecutionDeadline,
        ) -> Result<R>
        + 'static,
    ) -> Result<Target::Out<R>> {
        if Target::STAGED {
            let QueryExecution::Staged {
                transaction,
                origin,
            } = &self.session.execution
            else {
                unreachable!("staged sessions carry a transaction");
            };
            let validated = Arc::new(validated);
            let answer = transaction
                .lock()
                .expect("staged query lock poisoned")
                .stage_shared_match_with_limits(
                    &self.session.registry,
                    Arc::clone(&validated),
                    type_bridge_orm::MatchExecutionLimits::from_policy(
                        self.session.resources,
                        self.session.cancellation.clone(),
                    ),
                )?;
            let session = QuerySession::<S, Target> {
                cancellation: self.session.cancellation.clone(),
                installed: Arc::clone(&self.session.installed),
                execution: QueryExecution::Decode(origin.clone()),
                resources: self.session.resources,
                session: self.session.session.clone(),
                registry: Arc::clone(&self.session.registry),
                nonce: self.session.nonce,
                bindings: Vec::new(),
                closed: AtomicBool::new(false),
                marker: PhantomData,
            };
            let selection = self.selection.clone();
            return Ok(Target::pending(Pending {
                answer,
                decode: Box::new(move |result| {
                    let query = Query {
                        session: &session,
                        selection: selection.clone(),
                        hidden: Vec::new(),
                        predicates: Vec::new(),
                        allowed_cross_joins: Vec::new(),
                        closed: AtomicBool::new(false),
                    };
                    decode(
                        &query,
                        &validated,
                        result,
                        QueryExecutionDeadline::for_limits(session.resources),
                    )
                }),
            }));
        }
        self.ensure_open(ModelValidationPhase::Input)?;
        self.session
            .check_invocation(deadline, ModelValidationPhase::Input)?;
        let target = match &self.session.execution {
            QueryExecution::Staged { .. } | QueryExecution::Decode(_) => {
                unreachable!("staged queries execute at commit")
            }
            QueryExecution::Borrowed(transaction) => OrmQueryTarget::Transaction(transaction),
            QueryExecution::Local(database) => OrmQueryTarget::Database(database.inner_orm()),
            QueryExecution::Remote(remote) => {
                let result = remote
                    .execute_match(
                        &self.session.registry,
                        validated,
                        self.session.resources,
                        self.session.cancellation.clone(),
                        deadline,
                    )
                    .await?;
                self.ensure_open(ModelValidationPhase::Hydration)?;
                let (validated, result) = result;
                return decode(self, &validated, &result, deadline).map(Target::immediate);
            }
        };
        let result = target
            .execute(
                &self.session.registry,
                validated,
                self.session.resources,
                &self.session.cancellation,
                deadline,
            )
            .await;
        self.session
            .check_invocation(deadline, ModelValidationPhase::Hydration)?;
        self.ensure_open(ModelValidationPhase::Hydration)?;
        let (validated, result) = result.map_err(crate::remote::target_error)?;
        decode(self, &validated, &result, deadline).map(Target::immediate)
    }
}
