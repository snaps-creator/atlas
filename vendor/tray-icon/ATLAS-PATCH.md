# Atlas tray-icon patch

Upstream: tauri-apps/tray-icon0.25.1, MIT OR Apache-2.0.
Registry archive SHA-256: b2b9c52859a94554803ccd4a24b98f74148ebc73b90676d783f3490b1bff9d72.

The source was copied from that checksum-verified archive. Windows mouse callbacks must continue to deliver the actual event and open the existing native menu when Shell_NotifyIconGetRect is unavailable. The event keeps its real cursor position; an empty size explicitly reports unknown icon bounds. This does not create/register a tray icon or invoke a menu command directly.

Changes from upstream are limited to the Windows callback fallback and a private development feature `test-icon-rect-failure`. The isolated native fixture enables that feature to exercise the same callback failure path against a real registered icon and native menu. Release compilation with fault injection is rejected. Atlas never enables this feature.

To refresh upstream, preserve the failure regression and verify that the replacement version handles missing geometry before removing this patch. Other upstream sources and licenses are unchanged.
Hover dispatch still requires known bounds; the leave timer is canceled if cursor/bounds become unavailable so this fallback cannot leave an idle timer running.
