select stuff((
        select ',' + cfactionid as [commaWithContent] 
        from actions a
        where a.cfactionid is not null
        for xml path(''), type  
    ).value('.', 'nvarchar(max)'), 1, 1, '') existingActionIds