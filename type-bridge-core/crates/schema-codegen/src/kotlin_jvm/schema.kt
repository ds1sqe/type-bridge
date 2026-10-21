package __PACKAGE__

/** Opens the immutable authority emitted with this schema package. */
public object TypeBridgeSchema {
    internal fun bindingEvidence(): kotlin.ByteArray = kotlin.byteArrayOf(__BINDING__)

    @kotlin.jvm.JvmStatic
    public fun open(): io.github.ds1sqe.typebridge.SchemaPackage =
        io.github.ds1sqe.typebridge.SchemaPackage.openGenerated(
            TypeBridgeSchema::class.java,
            "__PACKAGE__",
            "/__RESOURCE_ROOT__/",
            kotlin.byteArrayOf(__AUTHORITY__),
            bindingEvidence()
        )
}
