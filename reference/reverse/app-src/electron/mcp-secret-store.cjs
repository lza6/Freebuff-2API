'use strict'

/**
 * The keyed secret store behind the consent bridge's `/secret` route.
 *
 * Ciphertext lives in one file in userData; the key never leaves the OS keychain (`safeStorage`
 * asks it for one under this app's identity). So what an attacker gets from the disk — a stolen
 * laptop, a backup, another account on the machine — is bytes they cannot read. That is the threat
 * this exists for; it does not, and cannot, defend against a process already running as the user.
 *
 * Values are encrypted individually rather than the file as a whole, so a corrupt or
 * undecryptable entry costs one connector's sign-in rather than everybody's.
 *
 * No top-level `require('electron')`: safeStorage and the path are injected, so this unit-tests
 * under Bun without an Electron runtime.
 */

const fs = require('node:fs')
const path = require('node:path')

/** @param safeStorage  Electron's, or anything with the same three methods. */
function createSecretStore({ safeStorage, file }) {
  // Availability is resolved on FIRST USE, never at construction.
  //
  // This is built during `boot()`, on the path that gates spawning the orchestrator, and
  // `isEncryptionAvailable()` reaches the OS keychain. On a machine whose login keychain is not
  // unlocked — a CI runner, a headless box, a fresh login — that call can block rather than
  // return false, and a blocked boot is an app that starts and never finishes starting. It cost
  // three packaged-restart e2e tests, each hanging its full 60s with the process alive and no
  // error anywhere, which is the least diagnosable shape a failure can take.
  //
  // Nothing needs the keychain until a connector actually signs in, which is long after boot. So
  // construction now touches nothing, and the first `/secret` request pays for the probe — where
  // a slow or hostile keychain costs one sign-in instead of the whole app.
  let availability = null
  const available = () => {
    if (availability === null) {
      try {
        // Not "is a keychain present" — `isEncryptionAvailable` is false on a Linux box with no
        // keyring, and on that machine the honest answer is that there is nowhere safe to put
        // this. A throw means the same thing: we cannot store secrets here.
        availability = safeStorage?.isEncryptionAvailable?.() === true
      } catch {
        availability = false
      }
    }
    return availability
  }

  const readAll = () => {
    try {
      const parsed = JSON.parse(fs.readFileSync(file, 'utf8'))
      return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : {}
    } catch {
      // Missing is the ordinary case on first run; unreadable means somebody edited it, and
      // refusing to start over would leave the user unable to sign in to anything.
      return {}
    }
  }

  const writeAll = (all) => {
    fs.mkdirSync(path.dirname(file), { recursive: true })
    const tmp = `${file}.tmp`
    fs.writeFileSync(tmp, JSON.stringify(all), { mode: 0o600 })
    fs.renameSync(tmp, file)
  }

  return {
    available,
    get(key) {
      if (!available()) return null
      const raw = readAll()[key]
      if (typeof raw !== 'string') return null
      try {
        return safeStorage.decryptString(Buffer.from(raw, 'base64'))
      } catch {
        // Written under a key this machine no longer has — a restored backup, a changed login
        // keychain. Unreadable is the same as absent: the user signs in again.
        return null
      }
    },
    set(key, value) {
      if (!available()) throw new Error('this host has no secure storage')
      writeAll({ ...readAll(), [key]: safeStorage.encryptString(value).toString('base64') })
    },
    delete(key) {
      if (!available()) return
      const all = readAll()
      if (!(key in all)) return
      delete all[key]
      writeAll(all)
    },
  }
}

module.exports = { createSecretStore }
