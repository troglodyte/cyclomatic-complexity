rootProject.name = "jscx-intellij"

plugins {
    // Lets Gradle provision the JDK 21 toolchain the IntelliJ Platform needs,
    // rather than requiring it to be installed and JAVA_HOME'd by hand.
    // 1.0.0 or newer is required on Gradle 9: earlier versions reference
    // JvmVendorSpec.IBM_SEMERU, which Gradle 9 removed.
    id("org.gradle.toolchains.foojay-resolver-convention") version "1.0.0"
}
