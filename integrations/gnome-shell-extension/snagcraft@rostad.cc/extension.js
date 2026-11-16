/* SnagCraft Capture — GNOME Shell panel companion.
 *
 * SnagCraft itself is a Tauri app and cannot run inside the shell process;
 * this thin extension is the classic bridge: a panel button that asks the
 * running SnagCraft instance to capture (single-instance plugin forwards
 * `--capture` / `--region` as events, or starts the app if it is not running).
 *
 * Gestures:
 *   left click   → capture the full screen
 *   middle click → capture a region
 *   right click  → open the SnagCraft editor
 *
 * Requires `snagcraft` on PATH (symlink the AppImage into ~/.local/bin,
 * or install the .deb). GNOME Shell 45+ (ESM).
 */

import GObject from 'gi://GObject';
import St from 'gi://St';
import Gio from 'gi://Gio';
import Clutter from 'gi://Clutter';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const SnagCraftButton = GObject.registerClass(
class SnagCraftButton extends St.Bin {
    _init() {
        super._init({
            style_class: 'panel-button snagcraft-panel-button',
            reactive: true,
            can_focus: true,
            track_hover: true,
            accessible_name: 'SnagCraft — capture the screen',
        });
        this.set_child(new St.Icon({
            icon_name: 'camera-photo-symbolic',
            style_class: 'system-status-icon',
        }));
        this.connect('button-press-event', (_actor, event) => {
            switch (event.get_button()) {
            case 1:
                this._launch('--capture');
                return Clutter.EVENT_STOP;
            case 2:
                this._launch('--region');
                return Clutter.EVENT_STOP;
            case 3:
                this._launch(null);
                return Clutter.EVENT_STOP;
            default:
                return Clutter.EVENT_PROPAGATE;
            }
        });
    }

    _launch(flag) {
        const argv = flag ? ['snagcraft', flag] : ['snagcraft'];
        try {
            Gio.Subprocess.new(argv, Gio.SubprocessFlags.NONE);
        } catch (e) {
            logError(e, 'SnagCraft extension');
            Main.notifyError('SnagCraft',
                'Could not launch SnagCraft. Is `snagcraft` on your PATH? ' +
                'See integrations/README.md in the SnagCraft repo.');
        }
    }
});

export default class SnagCraftExtension extends Extension {
    enable() {
        this._button = new SnagCraftButton();
        Main.panel.addToStatusArea(this.uuid, this._button, 0, 'right');
    }

    disable() {
        this._button?.destroy();
        this._button = null;
    }
}
