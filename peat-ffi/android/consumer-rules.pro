# JNI resolves this interface and callback method by their binary names.
-keep interface com.defenseunicorns.peat.OutboundFrameListener { *; }
-keepclassmembers class * implements com.defenseunicorns.peat.OutboundFrameListener {
    public void onFrame(java.lang.String, java.lang.String, byte[]);
}
-keep interface com.defenseunicorns.peat.DocumentChangeListener { *; }
-keepclassmembers class * implements com.defenseunicorns.peat.DocumentChangeListener {
    public void onChange(java.lang.String, java.lang.String);
    public void onError(java.lang.String);
}
