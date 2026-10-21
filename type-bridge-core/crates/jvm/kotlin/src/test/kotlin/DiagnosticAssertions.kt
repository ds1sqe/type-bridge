import io.github.ds1sqe.typebridge.*
import java.math.BigInteger

object DiagnosticAssertions {
    @JvmStatic fun verifyCategory(error: SdkExecutionException, expected: String) {
        check(error.diagnostic.version == 1)
        check(error.diagnostic.category.name.lowercase(java.util.Locale.ROOT) == expected)
    }
    @JvmStatic fun verify(error: SdkExecutionException) {
        val d = error.diagnostic
        val maximum = BigInteger("18446744073709551615")
        check(d.version == 1 && d.category == ExecutionCategory.INVALID_INPUT)
        check(d.code == "fixture_error" && d.message == "Fixture diagnostic")
        check(error.message == d.message)
        val person = DiagnosticType("entity", "person")
        val field = DiagnosticField(person, "name")
        val role = DiagnosticRole("membership", "member")
        check(d.path == listOf(
            DiagnosticPath.Argument("input"), DiagnosticPath.Index(maximum),
            DiagnosticPath.Type(person), DiagnosticPath.Field(field), DiagnosticPath.Role(role),
            DiagnosticPath.Query("request"), DiagnosticPath.Binding(65535),
            DiagnosticPath.QueryField("entity:person", "displayName"),
            DiagnosticPath.QueryRole("relation:membership", "member"),
            DiagnosticPath.RoleEdge(65535), DiagnosticPath.OutputSlot(maximum),
            DiagnosticPath.OutputName("result"), DiagnosticPath.ContractField("schema"),
            DiagnosticPath.ContractIdentity("entity:person")
        ))
        check(d.details.size == 16)
        check(d.details["boolean"] == DiagnosticDetail.BooleanValue(true))
        check(d.details["count"] == DiagnosticDetail.Count(maximum))
        check(d.details["bytes"] == DiagnosticDetail.ByteCount(maximum))
        check(d.details["signed"] == DiagnosticDetail.Signed(Long.MIN_VALUE))
        check(d.details["capability"] == DiagnosticDetail.Capability("schema.annotations"))
        check(d.details["value_type"] == DiagnosticDetail.ValueType("string"))
        check(d.details["type"] == DiagnosticDetail.Type(person))
        check(d.details["field"] == DiagnosticDetail.Field(field))
        check(d.details["role"] == DiagnosticDetail.Role(role))
        check(d.details["provider"] == DiagnosticDetail.ProviderOperation("write"))
        check(d.details["commit"] == DiagnosticDetail.CommitOutcome("unknown"))
        check(d.details["query_category"] == DiagnosticDetail.QueryCategory("invalid_plan"))
        check(d.details["identity"] == DiagnosticDetail.QueryIdentity("entity:person"))
        val fingerprint = (d.details["fingerprint"] as DiagnosticDetail.Fingerprint).identity
        check(fingerprint.domain == "fixture.diagnostic" && fingerprint.algorithm == "sha256")
        check(fingerprint.canonicalization == "typebridge.canonical-json/v1")
        check(fingerprint.semanticProfile == "typedb-3.12.1/v1" && fingerprint.digest == "a".repeat(64))
        check((d.details["unprofiled"] as DiagnosticDetail.Fingerprint).identity.semanticProfile == null)
        val identities = (d.details["identities"] as DiagnosticDetail.QueryIdentityList).values
        check(identities == (0..31).map { "entity:person$it" })
        try { (d.path as MutableList).clear(); error("mutable path") } catch (_: UnsupportedOperationException) {}
        try { (d.details as MutableMap).clear(); error("mutable details") } catch (_: UnsupportedOperationException) {}
        try { (identities as MutableList).clear(); error("mutable identity list") } catch (_: UnsupportedOperationException) {}
    }
}
