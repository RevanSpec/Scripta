La suite
Vous, dans cet ordre

Supprimer le brouillon v0.1.0.
Poser le tag v0.1.1 sur main, puis relire le brouillon créé et le publier :
Fusionner ensuite la #10. La branche claude/release-v0.1.1 ne sert plus, vous pouvez la supprimer.
Moi, ensuite (dans l'ordre que je recommande) :

Essayer les installeurs pour de vrai.
Linux : je peux lancer la version AppImage dans le WSL Ubuntu de votre poste, sans rien y installer. Ce serait la première fois que l'application tourne sous Linux, un critère du J3.
Windows : l'installeur s'installe dans votre profil, sans droits d'administrateur, mais il faut votre accord.
macOS : impossible sans Mac.
Publier une v0.2.0, première version avec l'application de bureau (notes de version, numéro de version).
Une version qui utilise la carte graphique. Votre portable a une carte NVIDIA. C'est l'occasion de mesurer enfin la transcription sur GPU et de valider l'accélération affichée, dernier critère du J3 qui manque d'une machine. Il faut installer le SDK Vulkan via winget, avec votre accord.
Deux automatismes sur GitHub :
une vérification quotidienne que yt-dlp fonctionne toujours face à YouTube ;
un suivi des performances à chaque modification.
Un binaire de la CLI pour les Mac Intel, qui n'en ont pas aujourd'hui.
Une fois ces étapes faites, les jalons 3 et 4 seront clos, à deux réserves près : l'application n'aura jamais été lancée sous macOS, et ses installeurs resteront non signés. Ce sera l'état de la v1.0.