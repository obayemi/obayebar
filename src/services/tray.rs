use futures_util::stream::StreamExt;
use futures_util::Stream;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrayItemInfo {
    pub id: String,
    pub title: String,
    pub icon_name: String,
    pub bus_name: String,
    pub path: String,
}

pub fn activate_item(id: &str) {
    let id = id.to_string();
    tokio::spawn(async move {
        if let Err(e) = activate_item_dbus(&id).await {
            log::warn!("Failed to activate tray item {id}: {e}");
        }
    });
}

async fn build_proxy<'a>(
    conn: &'a zbus::Connection,
    dest: &str,
    path: &str,
    iface: &str,
) -> Option<zbus::Proxy<'a>> {
    zbus::proxy::Builder::new(conn)
        .destination(dest.to_string())
        .ok()?
        .path(path.to_string())
        .ok()?
        .interface(iface.to_string())
        .ok()?
        .build()
        .await
        .ok()
}

async fn activate_item_dbus(id: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (bus_name, path) = id.split_once(':').ok_or("invalid tray item id format")?;

    let conn = zbus::Connection::session().await?;
    let proxy: zbus::Proxy<'_> = zbus::proxy::Builder::new(&conn)
        .destination(bus_name.to_string())?
        .path(path.to_string())?
        .interface("org.kde.StatusNotifierItem".to_string())?
        .build()
        .await?;

    proxy.call_noreply("Activate", &(0i32, 0i32)).await?;
    Ok(())
}

async fn read_tray_items_with(conn: &zbus::Connection) -> Vec<TrayItemInfo> {
    let Some(watcher_proxy) = build_proxy(
        conn,
        "org.kde.StatusNotifierWatcher",
        "/StatusNotifierWatcher",
        "org.kde.StatusNotifierWatcher",
    )
    .await
    else {
        return Vec::new();
    };

    let items: Vec<String> = match watcher_proxy
        .get_property("RegisteredStatusNotifierItems")
        .await
    {
        Ok(items) => items,
        Err(_) => return Vec::new(),
    };

    let mut tray_items = Vec::new();

    for item_service in &items {
        let (bus_name, path) = if let Some((name, path)) = item_service.split_once('/') {
            (name.to_string(), format!("/{path}"))
        } else {
            (item_service.clone(), "/StatusNotifierItem".to_string())
        };

        let Some(item_proxy) =
            build_proxy(conn, &bus_name, &path, "org.kde.StatusNotifierItem").await
        else {
            continue;
        };

        let id: String = item_proxy
            .get_property("Id")
            .await
            .unwrap_or_else(|_| bus_name.clone());

        let title: String = item_proxy
            .get_property("Title")
            .await
            .unwrap_or_else(|_| id.clone());

        let icon_name: String = item_proxy
            .get_property("IconName")
            .await
            .unwrap_or_default();

        tray_items.push(TrayItemInfo {
            id: format!("{bus_name}:{path}"),
            title,
            icon_name,
            bus_name,
            path,
        });
    }

    tray_items
}

pub fn stream() -> impl Stream<Item = Vec<TrayItemInfo>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        loop {
            let conn = loop {
                if let Ok(c) = zbus::Connection::session().await {
                    break c;
                }
                log::warn!("tray: failed to connect to session D-Bus, retrying");
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            };

            if run_tray_loop(&conn, &tx).await.is_err() {
                log::warn!("tray: signal loop ended, reconnecting");
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        }
    });

    tokio_stream::wrappers::UnboundedReceiverStream::new(rx)
}

async fn run_tray_loop(
    conn: &zbus::Connection,
    tx: &tokio::sync::mpsc::UnboundedSender<Vec<TrayItemInfo>>,
) -> Result<(), ()> {
    let watcher_proxy = build_proxy(
        conn,
        "org.kde.StatusNotifierWatcher",
        "/StatusNotifierWatcher",
        "org.kde.StatusNotifierWatcher",
    )
    .await
    .ok_or(())?;

    // Subscribe to item registered/unregistered signals
    let mut registered = watcher_proxy
        .receive_signal("StatusNotifierItemRegistered")
        .await
        .map_err(|_| ())?;
    let mut unregistered = watcher_proxy
        .receive_signal("StatusNotifierItemUnregistered")
        .await
        .map_err(|_| ())?;

    // Emit initial state
    let items = read_tray_items_with(conn).await;
    tx.send(items).map_err(|_| ())?;

    loop {
        tokio::select! {
            Some(_) = registered.next() => {}
            Some(_) = unregistered.next() => {}
            // Fallback refresh every 2 minutes
            () = tokio::time::sleep(std::time::Duration::from_mins(2)) => {}
        }

        // Small delay to let D-Bus settle after registration changes
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let items = read_tray_items_with(conn).await;
        tx.send(items).map_err(|_| ())?;
    }
}
