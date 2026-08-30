use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiLanguage {
    English,
    PortugueseBrazil,
    French,
    Spanish,
    German,
    Japanese,
    ChineseSimplified,
}

#[derive(Debug, Clone, Copy)]
#[repr(usize)]
pub enum Text {
    StepStart,
    StepChoose,
    StepConfirm,
    Applying,
    Complete,
    Version,
    NoInstallTitle,
    NoInstallBody,
    NoInstallNoticeTitle,
    NoInstallNoticeBody,
    InstallGo,
    InstalledTitle,
    InstalledBody,
    InstalledNoticeTitle,
    InstalledNoticeBody,
    ManageGo,
    RepairTitle,
    RepairBody,
    RepairNoticeTitle,
    RepairNoticeBody,
    ReviewOptionsGo,
    ManageExisting,
    ReadyInstall,
    ExistingOptionsBody,
    NewOptionsBody,
    Install,
    Reinstall,
    InstallDescription,
    ReinstallDescription,
    RemoveFromComputer,
    RemoveDescription,
    Back,
    Review,
    ReadyReinstall,
    ReviewInstallBody,
    InstallNow,
    ReinstallNow,
    ReadyRemove,
    ReviewRemoveBody,
    RemoveNow,
    RemoveCertificate,
    RemoveCertificateDetail,
    MemoryNotice,
    Installing,
    Reinstalling,
    Removing,
    WindowStaysOpen,
    DoNotDisconnect,
    RemovedTitle,
    RemovedBody,
    CameraReadyTitle,
    CameraReadyBody,
    CameraPendingTitle,
    CameraPendingBody,
    InstallationPreparedTitle,
    InstallationPreparedBody,
    VerificationPendingTitle,
    VerificationPendingBody,
    InstallationFailed,
    RemovalFailed,
    Close,
    Preparing,
    Extracting,
    ApplyingChange,
    CheckingResult,
    Finalizing,
    ReviewMessage,
    ElevationFailed,
    NoUsableResult,
    EngineStartFailed,
    EngineEnded,
    PayloadUnavailable,
    Count,
}

const TEXT_COUNT: usize = Text::Count as usize;

impl UiLanguage {
    pub fn from_locale(locale: &str) -> Self {
        let locale = locale.trim().replace('_', "-").to_ascii_lowercase();
        match locale.as_str() {
            "fr-ca" | "fr-fr" => Self::French,
            value if value == "es" || value.starts_with("es-") => Self::Spanish,
            value if value.starts_with("pt-") => Self::PortugueseBrazil,
            value if value.starts_with("de-") || value == "de" => Self::German,
            value if value.starts_with("ja-") || value == "ja" => Self::Japanese,
            value if value.starts_with("zh-") || value == "zh" => Self::ChineseSimplified,
            _ => Self::English,
        }
    }

    pub fn system() -> Self {
        let mut locale = [0_u16; 85];
        let written = unsafe { GetUserDefaultLocaleName(locale.as_mut_ptr(), locale.len() as i32) };
        if written <= 1 {
            return Self::English;
        }
        let name = String::from_utf16_lossy(&locale[..written as usize - 1]);
        Self::from_locale(&name)
    }

    pub fn text(self, text: Text) -> &'static str {
        let index = text as usize;
        match self {
            Self::English => ENGLISH[index],
            Self::PortugueseBrazil => PORTUGUESE_BRAZIL[index],
            Self::French => FRENCH[index],
            Self::Spanish => SPANISH[index],
            Self::German => GERMAN[index],
            Self::Japanese => JAPANESE[index],
            Self::ChineseSimplified => CHINESE_SIMPLIFIED[index],
        }
    }
}

const ENGLISH: [&str; TEXT_COUNT] = [
    "1 of 3 · Get started", "2 of 3 · Choose", "3 of 3 · Confirm", "Applying changes", "Complete", "Version",
    "No installation was found.", "This wizard is ready to install the boot-mode driver, the PS5 Camera service, and diagnostics. The USB camera mode (05A9:058C) keeps using the Windows UVC driver.", "Ready to install", "You can review the operation before Windows asks for approval. No permanent firmware will be written.", "Install  ›",
    "PS5 Camera is already installed.", "A complete installation was found on this PC. You can reinstall its components or remove them; the Windows UVC driver stays untouched.", "Installation detected", "Choose Reinstall to apply this bundled version again, or Remove to remove only PS5 Camera components.", "Manage  ›",
    "An incomplete installation was found.", "The service and installation folder are not consistent. Reinstall restores the components; Remove removes them in a controlled way.", "Repair recommended", "No permanent firmware will be written. The UVC camera and other Windows drivers will not change.", "Review options  ›",
    "Manage the existing installation", "Ready to install", "Reinstall restores this version's components. Remove does not affect the Windows UVC camera.", "Review the installation before making changes to this PC.", "Install", "Reinstall", "Sets up boot mode 05A9:0580, the automatic service, and diagnostics.", "Restores boot mode 05A9:0580, the automatic service, and diagnostics.", "Remove from this PC", "Removes only PS5 Camera components. The Windows UVC driver is kept.", "‹ Back", "Review  ›",
    "Ready to reinstall", "Windows will ask for administrator approval to install the WinUSB driver only for USB Boot (05A9:0580), the PS5 Camera service, and diagnostics. The UVC camera (05A9:058C) will not change.", "Install now", "Reinstall now", "Ready to remove", "The PS5 Camera service and boot-mode WinUSB package will be removed. The UVC camera and other Windows drivers will remain.", "Remove now", "Also remove local trust for the development certificate", "Leave this clear if another PS5 Camera package from this publisher is still used on this PC.", "Firmware is loaded into the camera's memory only when it appears in USB Boot. Nothing is written permanently.",
    "Installing components", "Reinstalling components", "Removing components", "This window stays open until the operation is finished.", "Do not disconnect the camera while this change is in progress.",
    "Components removed", "Removal is complete. The Windows UVC camera was not changed.", "Camera ready", "Installation is complete. The camera reappeared as USB Camera-OV580 (05A9:058C) and can now be used by compatible apps.", "Installation complete; camera pending", "The driver and service were installed, but the camera remained in USB Boot (05A9:0580). Reconnect it to a USB 3.x port and check the PS5CameraService log if it does not reappear as a USB camera.", "Installation prepared", "The components are installed. Connect the camera directly to a USB 3.x port; the service will prepare it automatically when it appears.", "Installation complete; verification pending", "Windows did not provide a conclusive camera state. Installation is complete, but confirm that USB Camera-OV580 appears before using it.", "Installation could not be completed", "Removal could not be completed", "Close",
    "Preparing installation components...", "Extracting and verifying internal components...", "Components verified. Windows is applying the requested change...", "Checking the operation result...", "Finishing and checking the result...", "The operation finished with a message to review.", "Windows did not start administrator approval (code {code}).", "The operation ended without a usable result.", "The installation engine could not be started: {error}", "The installation engine ended with {status}.", "This build is for verification only and has no embedded installation payload.",
];

const PORTUGUESE_BRAZIL: [&str; TEXT_COUNT] = [
    "1 de 3 · Começar", "2 de 3 · Escolher", "3 de 3 · Confirmar", "Aplicando alterações", "Concluído", "Versão",
    "Nenhuma instalação foi encontrada.", "Este assistente está pronto para instalar o driver do modo boot, o serviço PS5 Camera e o diagnóstico. A câmera USB (05A9:058C) continua usando o driver UVC do Windows.", "Pronto para instalar", "Você poderá revisar a operação antes de autorizar o Windows. Nenhum firmware permanente será gravado.", "Instalar  ›",
    "PS5 Camera já está instalado.", "Uma instalação íntegra foi encontrada neste computador. Você pode reinstalar os componentes ou removê-los; o driver UVC do Windows não será alterado.", "Instalação detectada", "Escolha Reinstalar para aplicar novamente esta versão incorporada ou Remover para retirar somente os componentes PS5 Camera.", "Gerenciar  ›",
    "Foi encontrada uma instalação incompleta.", "O serviço e a pasta de instalação não estão consistentes. Reinstalar restaura os componentes; Remover os retira de maneira controlada.", "Reparo recomendado", "Nenhum firmware permanente será gravado. A câmera UVC e os outros drivers do Windows não serão alterados.", "Revisar opções  ›",
    "Gerenciar a instalação existente", "Pronto para instalar", "Reinstalar restaura os componentes desta versão. Remover não afeta a câmera UVC do Windows.", "Revise a instalação antes de qualquer alteração neste computador.", "Instalar", "Reinstalar", "Prepara o boot 05A9:0580, o serviço automático e o diagnóstico.", "Restaura o boot 05A9:0580, o serviço automático e o diagnóstico.", "Remover do computador", "Remove somente os componentes PS5 Camera. O driver UVC do Windows é preservado.", "‹ Voltar", "Revisar  ›",
    "Pronto para reinstalar", "O Windows pedirá autorização de administrador para instalar o driver WinUSB somente para USB Boot (05A9:0580), o serviço PS5 Camera e o diagnóstico. A câmera UVC (05A9:058C) não será alterada.", "Instalar agora", "Reinstalar agora", "Pronto para remover", "O serviço PS5 Camera e o pacote WinUSB do modo boot serão removidos. A câmera UVC e os demais drivers do Windows permanecerão.", "Remover agora", "Remover também a confiança local no certificado de desenvolvimento", "Deixe desmarcado se outro pacote PS5 Camera deste editor ainda for usado neste computador.", "O firmware é carregado somente na memória da câmera quando ela aparece em USB Boot. Nada é gravado permanentemente.",
    "Instalando componentes", "Reinstalando componentes", "Removendo componentes", "Esta janela permanecerá aberta até a operação terminar.", "Não desconecte a câmera enquanto esta alteração estiver em andamento.",
    "Componentes removidos", "A remoção foi concluída. A câmera UVC do Windows não foi alterada.", "Câmera pronta", "A instalação foi concluída. A câmera reapareceu como USB Camera-OV580 (05A9:058C) e já pode ser usada por aplicativos compatíveis.", "Instalação concluída; câmera pendente", "O driver e o serviço foram instalados, mas a câmera continuou em USB Boot (05A9:0580). Reconecte-a a uma porta USB 3.x e consulte o log PS5CameraService se ela não reaparecer como câmera USB.", "Instalação preparada", "Os componentes foram instalados. Conecte a câmera diretamente a uma porta USB 3.x; o serviço fará o preparo automático quando ela aparecer.", "Instalação concluída; verificação pendente", "O Windows não forneceu um estado conclusivo da câmera. A instalação foi concluída, mas confirme que USB Camera-OV580 aparece antes de usá-la.", "Não foi possível concluir a instalação", "Não foi possível concluir a remoção", "Fechar",
    "Preparando os componentes da instalação...", "Extraindo e verificando os componentes internos...", "Componentes verificados. O Windows está aplicando a alteração solicitada...", "Conferindo o resultado da operação...", "Finalizando e conferindo o resultado...", "A operação terminou com uma mensagem para revisar.", "O Windows não iniciou a autorização de administrador (código {code}).", "A operação terminou sem um resultado utilizável.", "Não foi possível iniciar o motor de instalação: {error}", "O motor de instalação terminou com {status}.", "Este build é somente para verificação e não possui payload de instalação incorporado.",
];

const FRENCH: [&str; TEXT_COUNT] = [
    "1 sur 3 · Commencer", "2 sur 3 · Choisir", "3 sur 3 · Confirmer", "Application des modifications", "Terminé", "Version",
    "Aucune installation n’a été trouvée.", "Cet assistant est prêt à installer le pilote du mode boot, le service PS5 Camera et le diagnostic. Le mode caméra USB (05A9:058C) continue d’utiliser le pilote UVC de Windows.", "Prêt à installer", "Vous pourrez vérifier l’opération avant que Windows demande votre autorisation. Aucun firmware permanent ne sera écrit.", "Installer  ›",
    "PS5 Camera est déjà installé.", "Une installation complète a été trouvée sur ce PC. Vous pouvez réinstaller ses composants ou les supprimer ; le pilote UVC de Windows reste inchangé.", "Installation détectée", "Choisissez Réinstaller pour appliquer à nouveau cette version intégrée, ou Supprimer pour retirer uniquement les composants PS5 Camera.", "Gérer  ›",
    "Une installation incomplète a été trouvée.", "Le service et le dossier d’installation ne sont pas cohérents. Réinstaller restaure les composants ; Supprimer les retire de manière contrôlée.", "Réparation recommandée", "Aucun firmware permanent ne sera écrit. La caméra UVC et les autres pilotes Windows ne seront pas modifiés.", "Voir les options  ›",
    "Gérer l’installation existante", "Prêt à installer", "Réinstaller restaure les composants de cette version. Supprimer n’affecte pas la caméra UVC de Windows.", "Vérifiez l’installation avant toute modification de ce PC.", "Installer", "Réinstaller", "Configure le mode boot 05A9:0580, le service automatique et le diagnostic.", "Restaure le mode boot 05A9:0580, le service automatique et le diagnostic.", "Supprimer de ce PC", "Supprime uniquement les composants PS5 Camera. Le pilote UVC de Windows est conservé.", "‹ Retour", "Vérifier  ›",
    "Prêt à réinstaller", "Windows demandera une autorisation administrateur pour installer le pilote WinUSB uniquement pour USB Boot (05A9:0580), le service PS5 Camera et le diagnostic. La caméra UVC (05A9:058C) ne sera pas modifiée.", "Installer maintenant", "Réinstaller maintenant", "Prêt à supprimer", "Le service PS5 Camera et le package WinUSB du mode boot seront supprimés. La caméra UVC et les autres pilotes Windows resteront en place.", "Supprimer maintenant", "Supprimer également la confiance locale dans le certificat de développement", "Laissez cette option décochée si un autre package PS5 Camera de cet éditeur est encore utilisé sur ce PC.", "Le firmware est chargé uniquement dans la mémoire de la caméra lorsqu’elle apparaît en USB Boot. Rien n’est écrit de façon permanente.",
    "Installation des composants", "Réinstallation des composants", "Suppression des composants", "Cette fenêtre reste ouverte jusqu’à la fin de l’opération.", "Ne débranchez pas la caméra pendant cette modification.",
    "Composants supprimés", "La suppression est terminée. La caméra UVC de Windows n’a pas été modifiée.", "Caméra prête", "L’installation est terminée. La caméra est réapparue comme USB Camera-OV580 (05A9:058C) et peut maintenant être utilisée par les applications compatibles.", "Installation terminée ; caméra en attente", "Le pilote et le service ont été installés, mais la caméra est restée en USB Boot (05A9:0580). Rebranchez-la sur un port USB 3.x et consultez le journal PS5CameraService si elle ne réapparaît pas comme caméra USB.", "Installation préparée", "Les composants sont installés. Branchez la caméra directement sur un port USB 3.x ; le service la préparera automatiquement lorsqu’elle apparaîtra.", "Installation terminée ; vérification en attente", "Windows n’a pas fourni un état concluant pour la caméra. L’installation est terminée, mais confirmez l’apparition de USB Camera-OV580 avant de l’utiliser.", "Impossible de terminer l’installation", "Impossible de terminer la suppression", "Fermer",
    "Préparation des composants d’installation...", "Extraction et vérification des composants internes...", "Composants vérifiés. Windows applique la modification demandée...", "Vérification du résultat de l’opération...", "Finalisation et vérification du résultat...", "L’opération s’est terminée avec un message à examiner.", "Windows n’a pas lancé la demande d’autorisation administrateur (code {code}).", "L’opération s’est terminée sans résultat utilisable.", "Impossible de démarrer le moteur d’installation : {error}", "Le moteur d’installation s’est terminé avec {status}.", "Cette build sert uniquement à la vérification et n’intègre aucun payload d’installation.",
];

const SPANISH: [&str; TEXT_COUNT] = [
    "1 de 3 · Empezar", "2 de 3 · Elegir", "3 de 3 · Confirmar", "Aplicando cambios", "Completado", "Versión",
    "No se encontró ninguna instalación.", "Este asistente instalará el controlador de modo de arranque, el servicio PS5 Camera y las herramientas de diagnóstico. El modo de cámara USB (05A9:058C) seguirá utilizando el controlador UVC de Windows.", "Listo para instalar", "Puede revisar la operación antes de que Windows solicite aprobación. No se escribirá firmware permanente.", "Instalar  ›",
    "PS5 Camera ya está instalado.", "Se encontró una instalación completa en este equipo. Puede reinstalar sus componentes o eliminarlos; el controlador UVC de Windows no se modificará.", "Instalación detectada", "Elija Reinstalar para aplicar de nuevo esta versión incluida, o Eliminar para quitar solo los componentes de PS5 Camera.", "Administrar  ›",
    "Se encontró una instalación incompleta.", "El servicio y la carpeta de instalación no son coherentes. Reinstalar restaura los componentes; Eliminar los quita de forma controlada.", "Se recomienda reparar", "No se escribirá firmware permanente. La cámara UVC y los demás controladores de Windows no cambiarán.", "Revisar opciones  ›",
    "Administrar la instalación existente", "Listo para instalar", "Reinstalar restaura los componentes de esta versión. Eliminar no afecta a la cámara UVC de Windows.", "Revise la instalación antes de realizar cambios en este equipo.", "Instalar", "Reinstalar", "Configura el modo de arranque 05A9:0580, el servicio automático y las herramientas de diagnóstico.", "Restaura el modo de arranque 05A9:0580, el servicio automático y las herramientas de diagnóstico.", "Eliminar de este equipo", "Elimina solo los componentes de PS5 Camera. Se conserva el controlador UVC de Windows.", "‹ Atrás", "Revisar  ›",
    "Listo para reinstalar", "Windows solicitará aprobación de administrador para instalar el controlador WinUSB solo para USB Boot (05A9:0580), el servicio PS5 Camera y las herramientas de diagnóstico. La cámara UVC (05A9:058C) no se modificará.", "Instalar ahora", "Reinstalar ahora", "Listo para eliminar", "Se eliminarán el servicio PS5 Camera y el paquete WinUSB del modo de arranque. La cámara UVC y los demás controladores de Windows permanecerán.", "Eliminar ahora", "Eliminar también la confianza local en el certificado de desarrollo", "Déjelo sin marcar si todavía se usa en este equipo otro paquete PS5 Camera de este editor.", "El firmware se carga en la memoria de la cámara solo cuando aparece como USB Boot. No se escribe nada de forma permanente.",
    "Instalando componentes", "Reinstalando componentes", "Eliminando componentes", "Esta ventana permanecerá abierta hasta que termine la operación.", "No desconecte la cámara mientras se realiza este cambio.",
    "Componentes eliminados", "La eliminación se completó. La cámara UVC de Windows no se modificó.", "Cámara lista", "La instalación se completó. La cámara reapareció como USB Camera-OV580 (05A9:058C) y ahora se puede usar con aplicaciones compatibles.", "Instalación completada; cámara pendiente", "El controlador y el servicio se instalaron, pero la cámara permaneció en USB Boot (05A9:0580). Vuelva a conectarla a un puerto USB 3.x y consulte el registro de PS5CameraService si no reaparece como cámara USB.", "Instalación preparada", "Los componentes están instalados. Conecte la cámara directamente a un puerto USB 3.x; el servicio la preparará automáticamente cuando aparezca.", "Instalación completada; verificación pendiente", "Windows no proporcionó un estado concluyente de la cámara. La instalación se completó, pero confirme que aparezca USB Camera-OV580 antes de usarla.", "No se pudo completar la instalación", "No se pudo completar la eliminación", "Cerrar",
    "Preparando los componentes de instalación...", "Extrayendo y verificando los componentes internos...", "Componentes verificados. Windows está aplicando el cambio solicitado...", "Comprobando el resultado de la operación...", "Finalizando y comprobando el resultado...", "La operación terminó con un mensaje para revisar.", "Windows no inició la solicitud de aprobación de administrador (código {code}).", "La operación terminó sin un resultado utilizable.", "No se pudo iniciar el motor de instalación: {error}", "El motor de instalación terminó con {status}.", "Esta compilación es solo para verificación y no contiene una carga de instalación integrada.",
];

const GERMAN: [&str; TEXT_COUNT] = [
    "1 von 3 · Start", "2 von 3 · Auswählen", "3 von 3 · Bestätigen", "Änderungen werden angewendet", "Abgeschlossen", "Version",
    "Keine Installation gefunden.", "Dieser Assistent installiert den Treiber für den Bootmodus, den PS5-Camera-Dienst und die Diagnose. Der USB-Kameramodus (05A9:058C) verwendet weiterhin den Windows-UVC-Treiber.", "Bereit zur Installation", "Sie können den Vorgang prüfen, bevor Windows um Ihre Zustimmung bittet. Es wird keine permanente Firmware geschrieben.", "Installieren  ›",
    "PS5 Camera ist bereits installiert.", "Auf diesem PC wurde eine vollständige Installation gefunden. Sie können die Komponenten neu installieren oder entfernen; der Windows-UVC-Treiber bleibt unverändert.", "Installation erkannt", "Wählen Sie Neu installieren, um diese integrierte Version erneut anzuwenden, oder Entfernen, um nur PS5-Camera-Komponenten zu entfernen.", "Verwalten  ›",
    "Eine unvollständige Installation wurde gefunden.", "Dienst und Installationsordner stimmen nicht überein. Neu installieren stellt die Komponenten wieder her; Entfernen entfernt sie kontrolliert.", "Reparatur empfohlen", "Es wird keine permanente Firmware geschrieben. Die UVC-Kamera und andere Windows-Treiber werden nicht geändert.", "Optionen prüfen  ›",
    "Vorhandene Installation verwalten", "Bereit zur Installation", "Neu installieren stellt die Komponenten dieser Version wieder her. Entfernen beeinflusst die Windows-UVC-Kamera nicht.", "Prüfen Sie die Installation, bevor Änderungen an diesem PC vorgenommen werden.", "Installieren", "Neu installieren", "Richtet Bootmodus 05A9:0580, den automatischen Dienst und die Diagnose ein.", "Stellt Bootmodus 05A9:0580, den automatischen Dienst und die Diagnose wieder her.", "Von diesem PC entfernen", "Entfernt nur PS5-Camera-Komponenten. Der Windows-UVC-Treiber bleibt erhalten.", "‹ Zurück", "Prüfen  ›",
    "Bereit zur Neuinstallation", "Windows fordert Administratorrechte an, um den WinUSB-Treiber nur für USB Boot (05A9:0580), den PS5-Camera-Dienst und die Diagnose zu installieren. Die UVC-Kamera (05A9:058C) wird nicht geändert.", "Jetzt installieren", "Jetzt neu installieren", "Bereit zum Entfernen", "Der PS5-Camera-Dienst und das WinUSB-Paket für den Bootmodus werden entfernt. Die UVC-Kamera und andere Windows-Treiber bleiben erhalten.", "Jetzt entfernen", "Lokales Vertrauen für das Entwicklungszertifikat ebenfalls entfernen", "Lassen Sie diese Option deaktiviert, wenn auf diesem PC noch ein anderes PS5-Camera-Paket dieses Herausgebers verwendet wird.", "Firmware wird nur in den Kameraspeicher geladen, wenn sie als USB Boot erscheint. Es wird nichts dauerhaft geschrieben.",
    "Komponenten werden installiert", "Komponenten werden neu installiert", "Komponenten werden entfernt", "Dieses Fenster bleibt geöffnet, bis der Vorgang abgeschlossen ist.", "Trennen Sie die Kamera nicht, während diese Änderung ausgeführt wird.",
    "Komponenten entfernt", "Das Entfernen ist abgeschlossen. Die Windows-UVC-Kamera wurde nicht geändert.", "Kamera bereit", "Die Installation ist abgeschlossen. Die Kamera ist wieder als USB Camera-OV580 (05A9:058C) verfügbar und kann nun von kompatiblen Apps verwendet werden.", "Installation abgeschlossen; Kamera ausstehend", "Treiber und Dienst wurden installiert, die Kamera blieb jedoch im USB Boot (05A9:0580). Verbinden Sie sie erneut mit einem USB-3.x-Anschluss und prüfen Sie das PS5CameraService-Protokoll, falls sie nicht als USB-Kamera erscheint.", "Installation vorbereitet", "Die Komponenten sind installiert. Schließen Sie die Kamera direkt an einen USB-3.x-Anschluss an; der Dienst bereitet sie automatisch vor, wenn sie erscheint.", "Installation abgeschlossen; Überprüfung ausstehend", "Windows hat keinen eindeutigen Kamerastatus geliefert. Die Installation ist abgeschlossen, bestätigen Sie aber vor der Verwendung, dass USB Camera-OV580 erscheint.", "Installation konnte nicht abgeschlossen werden", "Entfernen konnte nicht abgeschlossen werden", "Schließen",
    "Installationskomponenten werden vorbereitet...", "Interne Komponenten werden extrahiert und geprüft...", "Komponenten geprüft. Windows wendet die angeforderte Änderung an...", "Vorgangsergebnis wird geprüft...", "Ergebnis wird abgeschlossen und geprüft...", "Der Vorgang endete mit einer zu prüfenden Meldung.", "Windows hat die Administratorbestätigung nicht gestartet (Code {code}).", "Der Vorgang endete ohne verwendbares Ergebnis.", "Installations-Engine konnte nicht gestartet werden: {error}", "Die Installations-Engine endete mit {status}.", "Dieser Build dient nur zur Überprüfung und enthält kein eingebettetes Installationspaket.",
];

const JAPANESE: [&str; TEXT_COUNT] = [
    "3 ステップ中 1 · 開始", "3 ステップ中 2 · 選択", "3 ステップ中 3 · 確認", "変更を適用しています", "完了", "バージョン",
    "インストールは見つかりませんでした。", "このウィザードは、ブートモードのドライバー、PS5 Camera サービス、診断ツールをインストールします。USB カメラモード (05A9:058C) は Windows の UVC ドライバーを引き続き使用します。", "インストールの準備完了", "Windows が承認を求める前に操作内容を確認できます。永続的なファームウェアは書き込まれません。", "インストール  ›",
    "PS5 Camera はすでにインストールされています。", "この PC で完全なインストールが見つかりました。コンポーネントを再インストールまたは削除できます。Windows の UVC ドライバーは変更されません。", "インストールを検出", "この組み込みバージョンを再適用するには「再インストール」、PS5 Camera のコンポーネントだけを削除するには「削除」を選択してください。", "管理  ›",
    "不完全なインストールが見つかりました。", "サービスとインストールフォルダーが一致していません。再インストールでコンポーネントを復元し、削除で安全に削除します。", "修復を推奨", "永続的なファームウェアは書き込まれません。UVC カメラや他の Windows ドライバーは変更されません。", "オプションを確認  ›",
    "既存のインストールを管理", "インストールの準備完了", "再インストールはこのバージョンのコンポーネントを復元します。削除しても Windows の UVC カメラには影響しません。", "この PC を変更する前にインストール内容を確認してください。", "インストール", "再インストール", "ブートモード 05A9:0580、自動サービス、診断ツールを設定します。", "ブートモード 05A9:0580、自動サービス、診断ツールを復元します。", "この PC から削除", "PS5 Camera のコンポーネントだけを削除します。Windows の UVC ドライバーは保持されます。", "‹ 戻る", "確認  ›",
    "再インストールの準備完了", "Windows は USB Boot (05A9:0580) 専用の WinUSB ドライバー、PS5 Camera サービス、診断ツールをインストールするために管理者の承認を求めます。UVC カメラ (05A9:058C) は変更されません。", "今すぐインストール", "今すぐ再インストール", "削除の準備完了", "PS5 Camera サービスとブートモード用 WinUSB パッケージが削除されます。UVC カメラや他の Windows ドライバーは残ります。", "今すぐ削除", "開発証明書に対するローカルの信頼も削除する", "この発行元の別の PS5 Camera パッケージをこの PC で使用している場合は、選択しないでください。", "ファームウェアは、カメラが USB Boot として表示された場合にのみメモリへ読み込まれます。永続的な書き込みは行われません。",
    "コンポーネントをインストールしています", "コンポーネントを再インストールしています", "コンポーネントを削除しています", "操作が終了するまでこのウィンドウは開いたままになります。", "変更中はカメラを取り外さないでください。",
    "コンポーネントを削除しました", "削除が完了しました。Windows の UVC カメラは変更されていません。", "カメラの準備完了", "インストールが完了しました。カメラは USB Camera-OV580 (05A9:058C) として再表示され、対応アプリで使用できます。", "インストール完了；カメラ待機中", "ドライバーとサービスはインストールされましたが、カメラは USB Boot (05A9:0580) のままです。USB 3.x ポートへ再接続し、USB カメラとして再表示されない場合は PS5CameraService ログを確認してください。", "インストールの準備完了", "コンポーネントがインストールされました。カメラを USB 3.x ポートに直接接続してください。表示されるとサービスが自動的に準備します。", "インストール完了；確認待ち", "Windows はカメラの確定状態を返しませんでした。インストールは完了していますが、使用前に USB Camera-OV580 が表示されることを確認してください。", "インストールを完了できませんでした", "削除を完了できませんでした", "閉じる",
    "インストールコンポーネントを準備しています...", "内部コンポーネントを展開して確認しています...", "コンポーネントを確認しました。Windows が要求された変更を適用しています...", "操作結果を確認しています...", "結果を完了して確認しています...", "確認が必要なメッセージとともに操作が終了しました。", "Windows は管理者承認を開始しませんでした（コード {code}）。", "操作は使用可能な結果なしで終了しました。", "インストールエンジンを開始できませんでした: {error}", "インストールエンジンは {status} で終了しました。", "このビルドは検証専用で、埋め込みインストールペイロードを含みません。",
];

const CHINESE_SIMPLIFIED: [&str; TEXT_COUNT] = [
    "第 1 步（共 3 步）· 开始", "第 2 步（共 3 步）· 选择", "第 3 步（共 3 步）· 确认", "正在应用更改", "已完成", "版本",
    "未找到安装。", "此向导将安装启动模式驱动程序、PS5 Camera 服务和诊断工具。USB 摄像头模式 (05A9:058C) 将继续使用 Windows UVC 驱动程序。", "可以安装", "Windows 请求批准前，您可以查看操作内容。不会写入永久固件。", "安装  ›",
    "PS5 Camera 已安装。", "在此电脑上找到了完整安装。您可以重新安装组件或将其删除；Windows UVC 驱动程序不会改变。", "已检测到安装", "选择“重新安装”以再次应用此内置版本，或选择“删除”以仅删除 PS5 Camera 组件。", "管理  ›",
    "找到了不完整的安装。", "服务和安装文件夹不一致。重新安装将恢复组件；删除将以受控方式移除它们。", "建议修复", "不会写入永久固件。UVC 摄像头和其他 Windows 驱动程序不会改变。", "查看选项  ›",
    "管理现有安装", "可以安装", "重新安装将恢复此版本的组件。删除不会影响 Windows UVC 摄像头。", "在更改此电脑之前，请查看安装内容。", "安装", "重新安装", "设置启动模式 05A9:0580、自动服务和诊断工具。", "恢复启动模式 05A9:0580、自动服务和诊断工具。", "从此电脑删除", "仅删除 PS5 Camera 组件。Windows UVC 驱动程序会保留。", "‹ 返回", "查看  ›",
    "可以重新安装", "Windows 将请求管理员批准，以仅为 USB Boot (05A9:0580) 安装 WinUSB 驱动程序、PS5 Camera 服务和诊断工具。UVC 摄像头 (05A9:058C) 不会改变。", "立即安装", "立即重新安装", "可以删除", "将删除 PS5 Camera 服务和启动模式 WinUSB 包。UVC 摄像头和其他 Windows 驱动程序将保留。", "立即删除", "同时删除对开发证书的本地信任", "如果此电脑仍在使用此发布者的其他 PS5 Camera 包，请保持未选中。", "仅当摄像头显示为 USB Boot 时，固件才会加载到其内存中。不会永久写入任何内容。",
    "正在安装组件", "正在重新安装组件", "正在删除组件", "此窗口将保持打开，直到操作完成。", "更改进行时请勿断开摄像头。",
    "组件已删除", "删除已完成。Windows UVC 摄像头未被更改。", "摄像头已就绪", "安装已完成。摄像头已重新显示为 USB Camera-OV580 (05A9:058C)，现在可由兼容应用使用。", "安装完成；等待摄像头", "驱动程序和服务已安装，但摄像头仍处于 USB Boot (05A9:0580)。请将其重新连接到 USB 3.x 端口；如果未显示为 USB 摄像头，请检查 PS5CameraService 日志。", "安装已准备好", "组件已安装。请将摄像头直接连接到 USB 3.x 端口；显示后服务将自动准备它。", "安装完成；等待验证", "Windows 未提供确定的摄像头状态。安装已完成，但使用前请确认 USB Camera-OV580 已显示。", "无法完成安装", "无法完成删除", "关闭",
    "正在准备安装组件...", "正在提取并验证内部组件...", "组件已验证。Windows 正在应用请求的更改...", "正在检查操作结果...", "正在完成并检查结果...", "操作已结束，并有一条需要查看的消息。", "Windows 未启动管理员批准（代码 {code}）。", "操作结束时没有可用结果。", "无法启动安装引擎：{error}", "安装引擎以 {status} 结束。", "此构建仅用于验证，不包含嵌入式安装负载。",
];

#[cfg(test)]
mod tests {
    use super::UiLanguage;

    #[test]
    fn maps_only_the_requested_locales_and_uses_english_elsewhere() {
        assert_eq!(
            UiLanguage::from_locale("pt-BR"),
            UiLanguage::PortugueseBrazil
        );
        assert_eq!(
            UiLanguage::from_locale("pt-PT"),
            UiLanguage::PortugueseBrazil
        );
        assert_eq!(UiLanguage::from_locale("fr-CA"), UiLanguage::French);
        assert_eq!(UiLanguage::from_locale("fr-FR"), UiLanguage::French);
        assert_eq!(UiLanguage::from_locale("es-ES"), UiLanguage::Spanish);
        assert_eq!(UiLanguage::from_locale("es-MX"), UiLanguage::Spanish);
        assert_eq!(UiLanguage::from_locale("es-AR"), UiLanguage::Spanish);
        assert_eq!(UiLanguage::from_locale("de-DE"), UiLanguage::German);
        assert_eq!(UiLanguage::from_locale("ja-JP"), UiLanguage::Japanese);
        assert_eq!(
            UiLanguage::from_locale("zh-CN"),
            UiLanguage::ChineseSimplified
        );
        assert_eq!(
            UiLanguage::from_locale("zh-TW"),
            UiLanguage::ChineseSimplified
        );
        assert_eq!(UiLanguage::from_locale("fr-BE"), UiLanguage::English);
        assert_eq!(UiLanguage::from_locale("it-IT"), UiLanguage::English);
        assert_eq!(UiLanguage::from_locale("en-US"), UiLanguage::English);
    }
}
