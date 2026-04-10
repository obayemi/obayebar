use futures_util::Stream;

#[derive(Debug, Clone)]
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
    futures_util::stream::unfold(None, |conn: Option<zbus::Connection>| async {
        let connection = if let Some(c) = conn {
            c
        } else if let Ok(c) = zbus::Connection::session().await {
            c
        } else {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            return Some((Vec::new(), None));
        };
        let items = read_tray_items_with(&connection).await;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        Some((items, Some(connection)))
    })
}
