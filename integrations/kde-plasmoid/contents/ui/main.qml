/* SnagCraft Capture — KDE Plasma 6 panel companion.
 *
 * Same bridge as the GNOME extension: SnagCraft is a Tauri app, so the
 * plasmoid only asks the running instance to capture (`snagcraft --capture`
 * / `--region`; the single-instance plugin forwards it, or starts the app).
 *
 * Click the panel icon for a full-screen capture; expand the applet for
 * Region and Open editor. Requires `snagcraft` on PATH.
 */

import QtQuick
import QtQuick.Layouts
import org.kde.plasma.plasmoid
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami

PlasmoidItem {
    id: root

    preferredRepresentation: compactRepresentation

    PlasmaCore.DataSource {
        id: executable
        engine: "executable"
        connectedSources: []
    }

    function snagcraft(flag) {
        executable.connectSource(flag ? "snagcraft " + flag : "snagcraft");
    }

    compactRepresentation: Kirigami.Icon {
        source: "camera-photo"
        Layout.minimumWidth: Kirigami.Units.iconSizes.smallMedium
        Layout.minimumHeight: Kirigami.Units.iconSizes.smallMedium

        MouseArea {
            anchors.fill: parent
            onClicked: root.snagcraft("--capture")
        }
    }

    fullRepresentation: ColumnLayout {
        Layout.minimumWidth: Kirigami.Units.gridUnit * 12
        Layout.minimumHeight: Kirigami.Units.gridUnit * 7
        spacing: Kirigami.Units.smallSpacing

        PlasmaComponents.Label {
            Layout.alignment: Qt.AlignHCenter
            text: "SnagCraft"
            font.bold: true
        }
        PlasmaComponents.Button {
            Layout.fillWidth: true
            icon.name: "camera-photo"
            text: "Capture screen"
            onClicked: root.snagcraft("--capture")
        }
        PlasmaComponents.Button {
            Layout.fillWidth: true
            icon.name: "select-rectangular"
            text: "Capture region"
            onClicked: root.snagcraft("--region")
        }
        PlasmaComponents.Button {
            Layout.fillWidth: true
            icon.name: "document-open"
            text: "Open editor"
            onClicked: root.snagcraft("")
        }
    }
}
