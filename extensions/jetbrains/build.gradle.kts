plugins {
    // Kotlin 2.4 is the first release to support Gradle 9.x. Its tested ceiling
    // is Gradle 9.5, which is what the wrapper pins — see tasks.wrapper below.
    kotlin("jvm") version "2.4.10"
    id("org.jetbrains.intellij.platform") version "2.18.1"
}

group = providers.gradleProperty("pluginGroup").get()
version = providers.gradleProperty("pluginVersion").get()

repositories {
    mavenCentral()
    intellijPlatform {
        defaultRepositories()
    }
}

dependencies {
    intellijPlatform {
        intellijIdeaUltimate(providers.gradleProperty("platformVersion"))
    }

    // Plain JUnit 5, deliberately without TestFrameworkType.Platform: these tests
    // exercise pure functions and need no IDE fixture. Pulling in the platform
    // test framework registers com.intellij.tests.JUnit5TestSessionListener,
    // which cannot start outside a real platform test run.
    testImplementation(kotlin("test"))
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.4")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

intellijPlatform {
    pluginConfiguration {
        name = providers.gradleProperty("pluginName")
        version = providers.gradleProperty("pluginVersion")
        ideaVersion {
            sinceBuild = providers.gradleProperty("pluginSinceBuild")
            // Open-ended: this plugin is installed from disk, not distributed
            // through the marketplace, so pinning an upper bound only creates
            // needless breakage on the next IDE update.
            untilBuild = provider { null }
        }
    }
}

kotlin {
    jvmToolchain(21)
}

tasks.test {
    useJUnitPlatform()
}

tasks.wrapper {
    // Held at a Gradle version the Kotlin plugin actually supports, so
    // regenerating the wrapper does not silently move off it.
    gradleVersion = "9.5.0"
}
