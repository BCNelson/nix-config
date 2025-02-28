{ stdenv
, lib
, makeWrapper
, kdePackages
, coreutils
, bash
, libnotify
}:

stdenv.mkDerivation {
  pname = "kde-shred-menu";
  version = "0.1.0";

  dontUnpack = true;

  nativeBuildInputs = [ makeWrapper ];

  installPhase = ''
    # Create service menus directory
    mkdir -p $out/share/kio/servicemenus
    mkdir -p $out/bin

    # Create wrapper script
    cat > $out/bin/safe-remove.sh << EOF
    #!${bash}/bin/bash
    set -e

    if [ -z "\$1" ]; then
      ${kdePackages.kdialog}/bin/kdialog --error "No file selected"
      exit 1
    fi

    # Initialize arrays for tracking issues
    failed_files=()
    skipped_files=()
    total_files=0

    if ${kdePackages.kdialog}/bin/kdialog --title "Secure Delete" --warningcontinuecancel "Securely delete these files?\n\nThis action cannot be undone!"; then
      for file in "\$@"; do
        ((total_files++))
        
        if [ -d "\$file" ]; then
          skipped_files+=("\$file (Directory)")
          continue
        fi

        if [ ! -w "\$file" ]; then
          skipped_files+=("\$file (Permission denied)")
          continue
        fi

        if ! ${coreutils}/bin/shred -u -f -z -n3 "\$file" 2>/dev/null; then
          failed_files+=("\$file")
        fi
      done

      # Show error dialog if there were any issues
      if [ \''${#failed_files[@]} -gt 0 ] || [ \''${#skipped_files[@]} -gt 0 ]; then
        error_msg=""
        [ \''${#failed_files[@]} -gt 0 ] && error_msg="Failed to shred: \''${failed_files[*]}\n"
        [ \''${#skipped_files[@]} -gt 0 ] && error_msg="\$error_msg\nSkipped: \''${skipped_files[*]}"
        ${kdePackages.kdialog}/bin/kdialog --title "Secure Delete - Error" --error "\$error_msg"
      else
        # Only show notification on complete success
        ${libnotify}/bin/notify-send "Secure Delete" "Successfully shredded \$total_files file(s)" -i edit-delete-shred
      fi
    fi
    EOF

    chmod +x $out/bin/safe-remove.sh

    # Wrap the script to ensure it can find its runtime dependencies
    wrapProgram $out/bin/safe-remove.sh \
      --prefix PATH : ${lib.makeBinPath [ kdePackages.kdialog coreutils libnotify ]}

    # Create the .desktop file
    cat > $out/share/kio/servicemenus/shred.desktop << EOF
    [Desktop Entry]
    Type=Service
    ServiceTypes=KonqPopupMenu/Plugin
    MimeType=application/octet-stream;
    X-KDE-Priority=TopLevel
    Actions=Shred

    [Desktop Action Shred]
    Name=Secure Delete (Shred)
    Icon=edit-delete-shred
    Exec=$out/bin/safe-remove.sh %F
    EOF
  '';

  meta = with lib; {
    description = "KDE Dolphin service menu for secure file deletion";
    license = licenses.mit;
    platforms = platforms.linux;
    maintainers = [ ];
  };
}