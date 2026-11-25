package io.github.bstrauss.newspeak.devtool

import com.intellij.openapi.Disposable
import com.intellij.openapi.vfs.newvfs.impl.VfsRootAccess
import java.nio.file.Path

/**
 * Lets a light test's virtual file system reach the temporary directory by
 * its real path as well as its own: where the directory is reached through
 * a link -- macOS' `/var/folders`, which is `/private/var/folders` -- the
 * platform resolves the link, and the real path lies outside the roots a
 * light test may reach. Until [disposable] is disposed.
 */
fun allowTheTemporaryDirectory(disposable: Disposable) {
    val temporary = Path.of(System.getProperty("java.io.tmpdir"))
    VfsRootAccess.allowRootAccess(disposable, temporary.toString(), temporary.toRealPath().toString())
}
