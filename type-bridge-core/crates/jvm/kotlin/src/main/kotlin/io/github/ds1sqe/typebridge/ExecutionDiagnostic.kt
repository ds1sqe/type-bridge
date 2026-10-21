package io.github.ds1sqe.typebridge

import java.math.BigInteger
import java.util.Collections
import java.util.Locale

/** Stable, redacted engine failure categories. */
public enum class ExecutionCategory {
    INVALID_INPUT, UNSUPPORTED_CAPABILITY, RESOURCE_LIMIT, INTEGRITY,
    PROVIDER, TRANSACTION, CANCELLED, INTERNAL
}

public data class DiagnosticType(public val kind: String, public val label: String)
public data class DiagnosticField(public val owner: DiagnosticType, public val attribute: String)
public data class DiagnosticRole(public val relation: String, public val role: String)
public data class DiagnosticFingerprint(
    public val domain: String,
    public val algorithm: String,
    public val canonicalization: String,
    public val semanticProfile: String?,
    public val digest: String
)

/** Typed locations copied from Rust; counts preserve the entire unsigned range. */
public sealed interface DiagnosticPath {
    public data class Argument(public val name: String) : DiagnosticPath
    public data class Index(public val value: BigInteger) : DiagnosticPath
    public data class Type(public val identity: DiagnosticType) : DiagnosticPath
    public data class Field(public val identity: DiagnosticField) : DiagnosticPath
    public data class Role(public val identity: DiagnosticRole) : DiagnosticPath
    public data class Query(public val kind: String) : DiagnosticPath
    public data class Binding(public val value: Int) : DiagnosticPath
    public data class QueryField(public val owner: String, public val name: String) : DiagnosticPath
    public data class QueryRole(public val owner: String, public val name: String) : DiagnosticPath
    public data class RoleEdge(public val value: Int) : DiagnosticPath
    public data class OutputSlot(public val value: BigInteger) : DiagnosticPath
    public data class OutputName(public val name: String) : DiagnosticPath
    public data class ContractField(public val name: String) : DiagnosticPath
    public data class ContractIdentity(public val name: String) : DiagnosticPath
}

/** Closed detail alternatives preserve their engine meaning and numeric precision. */
public sealed interface DiagnosticDetail {
    public data class BooleanValue(public val value: Boolean) : DiagnosticDetail
    public data class Count(public val value: BigInteger) : DiagnosticDetail
    public data class ByteCount(public val value: BigInteger) : DiagnosticDetail
    public data class Capability(public val value: String) : DiagnosticDetail
    public data class ValueType(public val value: String) : DiagnosticDetail
    public data class Type(public val identity: DiagnosticType) : DiagnosticDetail
    public data class Field(public val identity: DiagnosticField) : DiagnosticDetail
    public data class Role(public val identity: DiagnosticRole) : DiagnosticDetail
    public data class Fingerprint(public val identity: DiagnosticFingerprint) : DiagnosticDetail
    public data class ProviderOperation(public val value: String) : DiagnosticDetail
    public data class CommitOutcome(public val value: String) : DiagnosticDetail
    public data class Signed(public val value: Long) : DiagnosticDetail
    public data class QueryCategory(public val value: String) : DiagnosticDetail
    public data class QueryIdentity(public val value: String) : DiagnosticDetail
    public class QueryIdentityList(values: Array<String>) : DiagnosticDetail {
        public val values: List<String> = java.util.List.copyOf(values.asList())
    }
}

/** One deterministic named diagnostic detail crossing JNI. */
public data class DiagnosticDetailEntry(public val name: String, public val value: DiagnosticDetail)

/** An immutable projection of one versioned Rust execution diagnostic. */
public class ExecutionDiagnostic internal constructor(
    public val version: Int,
    category: String,
    public val code: String,
    public val message: String,
    path: Array<DiagnosticPath>,
    details: Array<DiagnosticDetailEntry>
) {
    public val category: ExecutionCategory = ExecutionCategory.valueOf(category.uppercase(Locale.ROOT))
    public val path: List<DiagnosticPath> = java.util.List.copyOf(path.asList())
    public val details: Map<String, DiagnosticDetail> = Collections.unmodifiableMap(details.associate { it.name to it.value })
}

/** A shared-engine failure with typed, owned diagnostic evidence. */
public class SdkExecutionException(public val diagnostic: ExecutionDiagnostic) : RuntimeException(diagnostic.message)
