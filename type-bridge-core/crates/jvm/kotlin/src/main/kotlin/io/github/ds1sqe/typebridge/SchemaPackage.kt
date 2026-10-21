package io.github.ds1sqe.typebridge

import java.io.IOException
import java.lang.ref.Cleaner
import java.nio.file.Path
import java.util.concurrent.atomic.AtomicLong

/** Lossless canonical Rust diagnostics from a rejected package operation. */
public class SchemaPackageException(public val canonicalDiagnostics: String) :
    RuntimeException(canonicalDiagnostics)

/** Explicit loader for the in-development native JVM runtime. */
public object TypeBridgeRuntime {
    @JvmStatic
    public fun load(path: Path) {
        System.load(path.toAbsolutePath().normalize().toString())
        if (NativeBridge.abiVersion() != 3) throw LocalFailure.VERSION.exception()
    }
}

/** An admitted immutable schema package with explicit native ownership. */
public class SchemaPackage private constructor(nativeHandle: Long) : AutoCloseable {
    private val handle = AtomicLong(nativeHandle)
    private val cleanup = cleaner.register(this, PackageCleanup(handle))

    public val namespace: String get() = NativeBridge.namespace(openHandle())
    public val modelCount: Int get() = NativeBridge.modelCount(openHandle())
    public val authorityFingerprint: ByteArray get() = NativeBridge.authorityFingerprint(openHandle())
    public val semanticFingerprint: ByteArray get() = NativeBridge.semanticFingerprint(openHandle())
    public val bindingFingerprint: ByteArray get() = NativeBridge.bindingFingerprint(openHandle())

    /** Generated-code entry point; Rust checks exact package and attribute authority. */
    public fun materializeLongAttribute(binding: ByteArray, label: String, value: Long): Long =
        NativeBridge.longAttribute(openHandle(), binding, label.toByteArray(Charsets.UTF_8), value)

    /** Generated-code entry point; Rust checks exact package and attribute authority. */
    public fun materializeBooleanAttribute(binding: ByteArray, label: String, value: Boolean): Boolean =
        NativeBridge.booleanAttribute(openHandle(), binding, label.toByteArray(Charsets.UTF_8), value)

    /** Generated-code entry point; Rust checks finite values and projected constraints. */
    public fun materializeDoubleAttribute(binding: ByteArray, label: String, value: Double): Double =
        NativeBridge.doubleAttribute(openHandle(), binding, label.toByteArray(Charsets.UTF_8), value)

    /** Generated-code entry point; Rust rejects malformed Unicode and checks constraints. */
    public fun materializeStringAttribute(binding: ByteArray, label: String, value: String): String =
        NativeBridge.stringAttribute(openHandle(), binding, label.toByteArray(Charsets.UTF_8), value)

    override fun close() { cleanup.clean() }

    private fun openHandle(): Long = handle.get().also {
        if (it == 0L) throw LocalFailure.CLOSED.exception()
    }

    private class PackageCleanup(private val handle: AtomicLong) : Runnable {
        override fun run() {
            val value = handle.getAndSet(0)
            if (value != 0L) NativeBridge.closePackage(value)
        }
    }

    public companion object {
        private val cleaner = Cleaner.create()

        /** Code-generation entry point; every resource is verified again in Rust. */
        @JvmStatic
        public fun openGenerated(anchor: Class<*>, namespace: String, resourceRoot: String, expectedAuthority: ByteArray, expectedBinding: ByteArray): SchemaPackage {
            if (NativeBridge.abiVersion() != 3) throw LocalFailure.VERSION.exception()
            val maximum = NativeBridge.maximumEvidenceBytes()
            if (maximum <= 0 || maximum == Int.MAX_VALUE) throw LocalFailure.VERSION.exception()
            if (namespace.length > 255) throw LocalFailure.INPUT_LIMIT.exception()
            val authority = resource(anchor, resourceRoot, "schema-authority.json", maximum)
            val projection = resource(anchor, resourceRoot, "runtime-projection.json", maximum)
            val semantic = resource(anchor, resourceRoot, "semantic-fingerprint.json", maximum)
            val binding = resource(anchor, resourceRoot, "binding-fingerprint.json", maximum)
            val handle = NativeBridge.openPackage(namespace.toByteArray(Charsets.UTF_8), authority, projection, semantic, binding, expectedAuthority, expectedBinding)
            if (handle <= 0) throw LocalFailure.BOUNDARY.exception()
            try {
                return SchemaPackage(handle)
            } catch (failure: Throwable) {
                try { NativeBridge.closePackage(handle) } catch (cleanup: Throwable) { failure.addSuppressed(cleanup) }
                throw failure
            }
        }

        private fun resource(anchor: Class<*>, root: String, name: String, maximum: Int): ByteArray {
            try {
                val input = anchor.getResourceAsStream(root + name) ?: throw LocalFailure.MISSING_RESOURCE.exception()
                return input.use {
                    val bytes = it.readNBytes(maximum + 1)
                    if (bytes.size > maximum) throw LocalFailure.INPUT_LIMIT.exception()
                    bytes
                }
            } catch (_: IOException) {
                throw LocalFailure.RESOURCE_IO.exception()
            }
        }
    }
}

private enum class LocalFailure(val category: String, val code: String, val text: String) {
    VERSION("unsupported_capability", "jvm_abi_unsupported", "Native JVM boundary version is unsupported"),
    CLOSED("invalid_contract", "jvm_package_closed", "Native package handle is invalid or closed"),
    INPUT_LIMIT("resource_limit", "jvm_input_limit", "Native package input exceeds its byte ceiling"),
    MISSING_RESOURCE("invalid_contract", "jvm_package_resource_missing", "Generated schema resource is missing"),
    RESOURCE_IO("invalid_contract", "jvm_package_resource_io", "Generated schema resource could not be read"),
    BOUNDARY("invalid_contract", "jvm_boundary_failure", "A native JVM boundary operation failed");

    fun exception(): SchemaPackageException = SchemaPackageException(
        """[{"category":"$category","code":"$code","details":{},"message":"$text","path":[]}]"""
    )
}

internal object NativeBridge {
    @JvmStatic external fun abiVersion(): Int
    @JvmStatic external fun maximumEvidenceBytes(): Int
    @JvmStatic external fun openPackage(namespace: ByteArray, authority: ByteArray, projection: ByteArray, semantic: ByteArray, binding: ByteArray, expectedAuthority: ByteArray, expectedBinding: ByteArray): Long
    @JvmStatic external fun namespace(handle: Long): String
    @JvmStatic external fun modelCount(handle: Long): Int
    @JvmStatic external fun authorityFingerprint(handle: Long): ByteArray
    @JvmStatic external fun semanticFingerprint(handle: Long): ByteArray
    @JvmStatic external fun bindingFingerprint(handle: Long): ByteArray
    @JvmStatic external fun longAttribute(handle: Long, binding: ByteArray, label: ByteArray, value: Long): Long
    @JvmStatic external fun booleanAttribute(handle: Long, binding: ByteArray, label: ByteArray, value: Boolean): Boolean
    @JvmStatic external fun stringAttribute(handle: Long, binding: ByteArray, label: ByteArray, value: String): String
    @JvmStatic external fun doubleAttribute(handle: Long, binding: ByteArray, label: ByteArray, value: Double): Double
    @JvmStatic external fun closePackage(handle: Long)
}
