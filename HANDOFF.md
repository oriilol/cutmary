# Cutmary Handoff (`dev` -> CachyOS Verification -> `main`)

Este documento es el canal de comunicación entre el agente de desarrollo en **Colab** (rama `dev`) y el agente de verificación en **CachyOS x86_64 con GPU real** (que verifica `dev` y fusiona a `main`).

> **Regla de ramas**
> - `dev`: rama de desarrollo activo (Colab). Todos los parches nuevos se prueban primero aquí (headless Xvfb + software rendering Lavapipe/llvmpipe).
> - `main`: **solo cambios verificados en Linux real con GPU y sesión gráfica real**. Nunca desarrollar directamente en `main`.
> - `cachyos-fixes`: si durante la verificación en CachyOS hace falta arreglar algo específico de Linux real / GPU real, hacerlo en esta rama (a partir de `dev`), hacer push y documentarlo en [`## Resultados CachyOS`](#resultados-cachyos).

---

## Estado actual (CapCut 9.8.0.4799 x86_64)

- **Commit base de Rosemary**: `411f2844cbd44754f727c2ebd22d7a168a506097`
- **Hasta dónde llega CapCut en Colab (Xvfb + openbox + Lavapipe/llvmpipe)**:
  - Arranca completo, carga ~198 hilos y ~2 GiB RSS.
  - Renderiza la **ventana principal de inicio (Home)** con barra de título macOS y el diálogo modal **"Terms of Service and Privacy Policy"** (`Agree and continue`).
  - Resuelto el crash frecuente a los ~7s por corrupción de heap / double-free (Qt 6.2.2 `QRhiMetal` hacía `objc_release` sobre `[NSString stringWithUTF8String:"main0"]`).
  - Resuelto el crash de CEF (`SIGSEGV at 0x0` desde `Chromium Embedded Framework+0x46093c6`) implementando `posix_spawnattr_set_csm_np`; los procesos helper de CEF ya arrancan sin crashear.
  - **En curso en Colab**:
    - Pulsar **"Agree and continue"** -> **"+ New project"** para entrar al **Editor de Vídeo** y arreglar los siguientes selectores/símbolos/crashes.

---

## Qué ha cambiado en `dev` desde `main` (`9bbb3a4`)

1. **`fix(appkit): NSColorSpace` (`patches/04-appkit-coregraphics-globals.patch`)**:
   - Implementados `-[NSColorSpace initWithICCProfileData:]`, `-[NSColorSpace initWithCGColorSpace:]`, `ICCProfileData`, `colorSpaceModel`, `numberOfColorComponents`, `CGColorSpace`, y `dealloc`.
   - Espacios compartidos inmortales separados para Gray (`model=0, components=1`, `CGColorSpaceCreateDeviceGray`) y CMYK (`model=2, components=4`) frente a RGB (`model=1, components=3`).
   - `initWithICCProfileData:` devuelve `nil` (liberando `receiver`) si los datos son `nil`, vacíos o no son `CFData`, igual que en macOS.
2. **`fix(corefoundation): CFMachPortCreateRunLoopSource` (`patches/02-cf-foundation-iokit-coretext.patch`)**:
   - Exportado `CFMachPortCreateRunLoopSource` (evita `SIGSEGV at 0x0` en `libqcocoa.dylib+0x323f9`).
   - Desmontaje de `libqcocoa.dylib+0x31ec0..0x3240c` confirmó que `CGEventTapCreateForPid` devuelve `NULL` y Qt realmente usa `CVDisplayLink` + `CFRunLoopSource` v0 para updates/timers; para puertos no-NULL hay un fallback inerte marcado `STUB:`.
3. **`fix(foundation): NSData writeToFile/writeToURL` (`patches/02-cf-foundation-iokit-coretext.patch`)**:
   - Implementados `writeToFile:options:error:`, `writeToURL:atomically:`, `writeToURL:options:error:` y selectores Base64 (`initWithBase64EncodedData:options:`, `initWithBase64Encoding:`, `base64EncodedStringWithOptions:`, `base64EncodedDataWithOptions:`, `base64Encoding`).
   - `writeToURL:*` valida `[url isFileURL]`; para esquemas no-archivo devuelve `NO` con `NSCocoaErrorDomain` código `518` (`NSFileWriteUnsupportedSchemeError`).
4. **`fix(foundation): CFBundleGetAllBundles y NSBundle allBundles/allFrameworks` (`patches/02-...`, `patches/04-...`)**:
   - Implementado `CFBundleGetAllBundles` (evita `SIGSEGV` en `QtCore` `QLibraryInfo::path`).
   - `+[NSBundle allFrameworks]` devuelve solo bundles `.framework` (sin el bundle principal de la app); `+[NSBundle allBundles]` devuelve los no-framework (incluyendo el principal).
5. **`fix(darwin/mach,kqueue): MACH_SEND_TIMEOUT + kevent64 MACH_RCV_MSG` (`patches/01-darwin-gcd-mach-net-sandbox.patch`)**:
   - `crates/darwin/src/mach/wire.rs` respeta `MACH_SEND_TIMEOUT` usando `MSG_DONTWAIT` + espera acotada cuando el socket Unix dgram está lleno (`EAGAIN`).
   - `crates/darwin/src/kqueue.rs::kevent64` ahora implementa `MACH_RCV_MSG` real (recibe/drena el mensaje Mach en `(ext[0], ext[1])` con `mach_msg` no bloqueante y devuelve `mach_msg_return_t` en `event.fflags`). Antes `kevent64` descartaba `MACH_RCV_MSG`, haciendo que los 5 hilos `MessagePumpKqueue` de Chromium/TTNet girasen al 100% CPU sin drenar sus puertos y bloqueando `CrBrowserMain` (congelando la UI) en `sendmsg` (`unix_wait_for_peer`) al llenarse `max_dgram_qlen=10`.
6. **`fix(darwin/spawn): posix_spawnattr_set_csm_np` (`patches/01-...`, `patches/04-...`)**:
   - Implementado `posix_spawnattr_set_csm_np` (no-op aceptado en Linux, `EINVAL` si `attr == NULL`), resolviendo el `SIGSEGV at 0x0` desde `Chromium Embedded Framework+0x46093c6` al lanzar procesos helper de CEF.
7. **`fix(corefoundation,foundation): interning de NSString ASCII cortos` (`patches/02-cf-foundation-iokit-coretext.patch`)**:
   - Interning sin tope artificial como `__NSCFConstantString` inmortal para ASCII `<= 9` bytes únicamente cuando el receptor es exactamente la clase `NSString` en `+[NSString stringWithUTF8String:]` y `+[NSString stringWithCString:encoding:]` (8-bit).
   - `+[NSMutableString stringWithUTF8String:]` y `+[NSMutableString stringWithCString:encoding:]` siempre devuelven una copia mutable nueva (verificado con test en `crates/foundation/tests/string_init.rs`); subclases de `NSString` tampoco usan el interning.
   - Evita el double-free de Qt 6.2.2 `QRhiMetal` al liberar `[NSString stringWithUTF8String:"main0"]`.
8. **`fix(appkit): CALayer geometryFlipped/opaque/autoresizingMask y NSWindow keyViewSelectionDirection` (`patches/04-appkit-coregraphics-globals.patch`)**:
   - Añadidas propiedades reales `isGeometryFlipped`/`setGeometryFlipped:`/`contentsAreFlipped`, `isOpaque`/`setOpaque:`, `needsDisplayOnBoundsChange`/`setNeedsDisplayOnBoundsChange:`, `autoresizingMask`/`setAutoresizingMask:` en `CALayer` y `keyViewSelectionDirection` (más helpers de key-view loop marcados `STUB:`) en `NSWindow`, necesarios al desbloquear el renderizado de CEF.
9. **`fix(security): SecureTransport SSL* y SecPKCS12Import para libqsecuretransportbackend.dylib` (`patches/03-...`, `patches/04-...`)**:
   - Añadido `crates/security/src/ssl.rs` con `SSLContext` CF real (creación, cifrados soportados, setters/getters de estado/versión) y claves `kSecImport*`; handshake/read/write devuelven `errSSLClosedAbort` (`STUB:`) en lugar de crashear en `0x0` cuando Qt abre sockets TLS al crear un proyecto.
10. **`fix(appkit): NSWindow worksWhenModal y flags de panel` (`patches/04-appkit-coregraphics-globals.patch`)**:
    - Añadidos `worksWhenModal`, `setWorksWhenModal:`, `becomesKeyOnlyIfNeeded`, `setBecomesKeyOnlyIfNeeded:`, `isFloatingPanel`, `setFloatingPanel:` en `NSWindow`.
11. **`fix(foundation): NSLock y NSRecursiveLock reales` (`patches/02-cf-foundation-iokit-coretext.patch`)**:
    - Añadido `crates/foundation/src/lock.rs` con `pthread_mutex_t` real (normal para `NSLock`, `PTHREAD_MUTEX_RECURSIVE` para `NSRecursiveLock`), soportando `init`, `dealloc`, `lock`, `unlock`, `tryLock`, `lockBeforeDate:`, `name`, `setName:` (requerido por `libcccreator.dylib` al abrir el editor de vídeo).

---

## Lista "VERIFICAR EN LINUX REAL"

1. **`net.unix.max_dgram_qlen` y `CrBrowserMain` (`MACH_SEND_TIMEOUT`)**:
   - Comprobar valor de `sysctl net.unix.max_dgram_qlen` en CachyOS (por defecto 10 en Linux).
   - Verificar que `CrBrowserMain` no se queda colgado al abrir ventanas/webviews de CEF y comprobar si algún receptor Mach de Chromium/Rosemary deja mensajes sin drenar.
2. **Interning de `NSString` y `NSMutableString`**:
   - Ejecutar `cargo test -p corefoundation --lib` y `cargo test -p foundation --test string_init`.
   - Comprobar en sesión real prolongada que no hay double-frees al compilar shaders Metal ni fugas notables de memoria.
3. **Shaders Metal -> Vulkan (`QRhiMetal` / `EffectSDK`) con GPU real**:
   - En Colab solo tenemos Lavapipe (software). En CachyOS con GPU Vulkan real, comprobar que la ventana de inicio, los diálogos, el preview del editor y los efectos renderizan sin artefactos, sin errores de validación Vulkan ni cuelgues de GPU.
4. **`NSColorSpace` / `NSBundle` / `NSData` / `CFMachPortCreateRunLoopSource`**:
   - Verificar que colores/perfiles ICC se ven correctamente en pantalla real, que Qt carga plugins/traducciones desde los bundles correctos y que los archivos de configuración/proyectos de CapCut se escriben sin errores.
5. **Integridad de los parches**:
   - Confirmar que `./scripts/build-rosemary.sh` aplica los 4 parches limpios sobre el commit `411f2844cbd44754f727c2ebd22d7a168a506097` y compila sin errores.

---

## Prompt listo para copiar y pegar al agente en CachyOS

```text
Eres el agente de verificación en Linux real (CachyOS x86_64 con GPU y sesión gráfica real) para el repositorio `cutmary`.

REGLAS ESTRICTAS:
- NO desarrolles directamente en `main`.
- `main` solo debe recibir merges de `dev` una vez verificados en Linux real.
- Si encuentras un fallo que requiere cambios de código, crea/usa la rama `cachyos-fixes` a partir de `dev`, aplica el fix allí, regenera y valida los parches (`git apply --check`), haz push de `cachyos-fixes` a `origin`, y documéntalo en `HANDOFF.md`.
- NUNCA subas el DMG, CapCut.app, binarios de CapCut, logs enormes ni credenciales. No abras PRs ni issues ni hagas push a `microlift/rosemary`.

PASOS A EJECUTAR:
1. Actualiza el repo y lee `HANDOFF.md` completo:
   git fetch origin
   git checkout dev
   git pull origin dev

2. Compila Rosemary con los parches de `dev` en un directorio de trabajo local (p. ej. `~/cutmary-work`) y verifica los tests unitarios clave:
   ./scripts/build-rosemary.sh ~/cutmary-work
   cd ~/cutmary-work/rosemary-src
   cargo test -p corefoundation --lib -- a_collection_holds_a_reference_of_its_own
   cargo test -p foundation --test string_init

3. Comprueba los parámetros del kernel/GPU:
   uname -a
   sysctl net.unix.max_dgram_qlen
   vulkaninfo --summary | head -n 40

4. Lanza CapCut en tu sesión gráfica real (con GPU Vulkan real, SIN `LIBGL_ALWAYS_SOFTWARE=1`) y guarda el log:
   ROSEMARY_KEEP_FAULT_HANDLER=1 ~/cutmary-work/rosemary-src/target/release/rosemary <RUTA_A_CapCut.app> 2>&1 | tee /tmp/capcut-cachyos.log

5. Verifica los puntos de la sección "Lista VERIFICAR EN LINUX REAL" de `HANDOFF.md`:
   - ¿Arranca de forma estable 3 veces seguidas sin double-free ni deadlock en `CrBrowserMain`?
   - ¿Se renderiza bien la ventana principal, el modal de Terms of Service y (si pulsas Agree -> New project) el editor de vídeo con GPU Vulkan real?
   - ¿Aparece el crash intermitente `Chromium Embedded Framework+0x46093c6` o algún otro selector/símbolo/crash?

6. Documenta los resultados (fecha, commit probado, kernel, GPU, qué funcionó, qué falló, extracto corto de errores si los hubo) en la sección `## Resultados CachyOS` de `HANDOFF.md`:
   - Si TODO pasa en Linux real:
     * Haz commit de la actualización de `HANDOFF.md` en `dev` y `git push origin dev`.
     * Fusiona `dev` en `main` (`git checkout main && git pull origin main && git merge --ff-only dev` o `--no-ff`, y `git push origin main`).
   - Si algo falla o requiere ajuste:
     * Crea/actualiza la rama `cachyos-fixes` desde `dev` con tus fixes y la actualización de `HANDOFF.md`, y haz `git push origin cachyos-fixes` (sin tocar `main`).
```

---

## Resultados CachyOS

### 2026-11-25 — CachyOS x86_64, KDE Plasma Wayland 2560×1440 @ 1.25×, NVIDIA GeForce RTX 5060 Ti

Rosemary base `411f2844`, parches `patches/01`–`05` regenerados desde el árbol verificado
(`git apply --check` limpio sobre la base y los 76 ficheros resultantes idénticos byte a byte).

Lanzamiento:

```bash
ROSEMARY_DECORATIONS=server ROSEMARY_MENU=none ROSEMARY_KEEP_FAULT_HANDLER=1 \
  rosemary run --new-instance CapCut.app
```

Antes de cada lanzamiento hay que borrar los `*.locked` de 0 bytes de
`com.lveditor.draft/*/` (el launcher lo hace solo); un cierre anómalo de CapCut los deja.

**Verificado en vivo**

- Arranque, Home, editor y apertura de proyectos existentes (`1125 (7)`, `1125 (8)`).
- Escala fraccional Wayland 1.25× sin borde ni área 80 %×80 %.
- Ventana maximizada sin franja negra superior: CapCut añade un accesorio de barra de título
  de 36 pt y Rosemary encogía con él el content view `FullSizeContentView`; en AppKit el
  accesorio se superpone.
- Importar una foto `.jpg`: aparece en Medios, en la línea de tiempo con miniaturas y llega
  al reproductor; el panel de Efectos se abre.

**Arreglos de esta tanda** (en `04`/`05`)

- `CVMetalTextureCache*` y `CVOpenGLTextureCache*` reales (crash en `RendererMetalV2::convertBuffer`).
- `-[MTLDevice newRenderPipelineStateWithDescriptor:options:reflection:error:]`.
- `kCVImageBufferAlphaChannelModeKey` / `_StraightAlpha` (débiles; CapCut los desreferencia).
- QuickLook mínimo: `QLThumbnailImageCreate` (devuelve NULL, como sin generador) y `kQLThumbnailOptionIconModeKey`.
- MSL §5.2.1: argumentos sin `[[buffer/texture/sampler]]` toman el primer índice libre (los shaders de SPIRV-Cross de CapCut).
- `_Unwind_*` y ABI C++ de imágenes cargadas con `dlopen` ligados al libunwind/libc++abi del
  runtime: iban al `libgcc_s` del host, que no lee el compact unwind de macOS y abortaba
  (`libres_pool.dylib`, comprobado en el core dump).

**Segunda tanda (misma tarde)**

Verificado en vivo: importar varias fotos seguidas, miniaturas en el panel de Medios y en la
línea de tiempo (4 clips), botón Export activo. El panel de Medios ya no se atasca en bucle.

- MSL `isunordered`/`isordered`: los dos shaders de medios de CapCut fallaban al traducirse y
  CapCut los pedía sin parar (63 787 + 2 083 traducciones fallidas en ~20 s; caché 0 escrituras).
- SPIR-V: `Ordered`/`Unordered` se emiten con `OpIsNan` (`OpOrdered`/`OpUnordered` son de
  `Kernel` y el compilador de NVIDIA se caía con ellos); validado con `spirv-val --target-env vulkan1.3`.
- `-[MTLCommandEncoder device]`.
- Ventanas no opacas: `-isOpaque`/`-setOpaque:` reales, superficie con alfa y swapchain
  `PRE_MULTIPLIED` cuando el invitado la pide (el menú de Qt usa una ventana transparente a pantalla completa).
- Diagnóstico con variables de entorno: `ROSEMARY_TRACE_SIGNALS`, `ROSEMARY_TRACE_FRAMES`,
  `ROSEMARY_TRACE_SHADER_CACHE`.

**Pendiente**

- El editor sigue yendo con retardo al cargar contenido: CPU (~7 %) y GPU (~1 %) casi ociosas,
  así que es espera, no cómputo; por localizar.
- Menú contextual: en Wayland la ventana transparente a pantalla completa de Qt no se puede
  colocar en (0,0) y sale gris/negra o como ventana aparte. En X11 (XWayland) sale en su sitio
  pero con fondo gris, y el editor aborta al compartir un contexto GL entre hilos (GLX).
- La pantalla de inicio no muestra miniaturas de proyectos ni el degradado del botón.
- El reproductor no reproduce y el editor va muy lento (~1 fps).
- Vídeo `.mp4`: faltan `CV{ColorPrimaries,TransferFunction,YCbCrMatrix}GetStringForIntegerCodePoint`
  (FFmpeg/libavutil), `CVBufferHasAttachment`, `CVImageBufferGetCleanRect`,
  `CVPixelBufferCreateWithIOSurface`, `CVIsCompressedPixelFormatAvailable`,
  `CVDisplayLinkGetCurrentTime` y `CVDisplayLinkGetCurrentCGDisplay`.
- `frameworks::tests` informa 8 símbolos duplicados (`_CFNotificationCenter*`, `_kCFProxyTypeNone`,
  `_NSURLTypeIdentifierKey`, `_kCGImageSourceTypeIdentifierHint`); dos tests de appkit fallan
  (`a_panel_keeps_its_becomes_key_only_if_needed`, `metal_scene_tracks_sibling_order_hiding_and_removal`).
- Al cerrar la ventana CapCut sigue vivo (comportamiento macOS) y sus helpers CEF quedan huérfanos al salir.
